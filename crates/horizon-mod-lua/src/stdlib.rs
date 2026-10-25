//! What piccolo's standard library leaves out that mods reach for:
//! `string.format`, `string.rep`, `table.insert`, `table.remove` and
//! `table.concat`, as Lua 5.4 defines them (the subset of `format` below).
use crate::{function, lua_error, text};
use piccolo::{Context, Error, Table, Value};

/// Adds them to the `string` and `table` tables.
pub fn install(ctx: Context<'_>) -> Result<(), Error<'_>> {
    let string = match ctx.get_global("string") {
        Value::Table(t) => t,
        _ => {
            let t = Table::new(&ctx);
            ctx.set_global("string", t)?;
            t
        }
    };
    string.set(
        ctx,
        "format",
        function(ctx, |ctx, args| {
            let out = format(&args)
                .map_err(|e| lua_error(ctx, format!("bad argument to 'format' ({e})")))?;
            Ok(vec![text(ctx, &out)])
        }),
    )?;
    string.set(
        ctx,
        "rep",
        function(ctx, |ctx, args| {
            let s = string_arg(&args, 0).ok_or_else(|| {
                lua_error(ctx, "bad argument #1 to 'rep' (string expected)".into())
            })?;
            let n = args.get(1).and_then(|v| v.to_integer()).unwrap_or(0).max(0) as usize;
            let sep = string_arg(&args, 2).unwrap_or_default();
            if (s.len() + sep.len()).saturating_mul(n) > 1 << 20 {
                return Err(lua_error(ctx, "resulting string too large".into()));
            }
            Ok(vec![text(ctx, &vec![s; n].join(&sep))])
        }),
    )?;
    let table = match ctx.get_global("table") {
        Value::Table(t) => t,
        _ => {
            let t = Table::new(&ctx);
            ctx.set_global("table", t)?;
            t
        }
    };
    table.set(
        ctx,
        "insert",
        function(ctx, |ctx, args| {
            let Some(Value::Table(t)) = args.first().copied() else {
                return Err(lua_error(
                    ctx,
                    "bad argument #1 to 'insert' (table expected)".into(),
                ));
            };
            let n = t.length();
            match args.len() {
                2 => {
                    t.set(ctx, n + 1, args[1])?;
                }
                3 => {
                    let at = args[1]
                        .to_integer()
                        .filter(|&i| (1..=n + 1).contains(&i))
                        .ok_or_else(|| {
                            lua_error(
                                ctx,
                                "bad argument #2 to 'insert' (position out of bounds)".into(),
                            )
                        })?;
                    for i in (at..=n).rev() {
                        t.set(ctx, i + 1, t.get(ctx, i))?;
                    }
                    t.set(ctx, at, args[2])?;
                }
                _ => {
                    return Err(lua_error(
                        ctx,
                        "wrong number of arguments to 'insert'".into(),
                    ));
                }
            }
            Ok(vec![])
        }),
    )?;
    table.set(
        ctx,
        "remove",
        function(ctx, |ctx, args| {
            let Some(Value::Table(t)) = args.first().copied() else {
                return Err(lua_error(
                    ctx,
                    "bad argument #1 to 'remove' (table expected)".into(),
                ));
            };
            let n = t.length();
            let at = args.get(1).and_then(|v| v.to_integer()).unwrap_or(n);
            if n == 0 && args.len() < 2 {
                return Ok(vec![Value::Nil]);
            }
            if !(1..=n + 1).contains(&at) {
                return Err(lua_error(
                    ctx,
                    "bad argument #2 to 'remove' (position out of bounds)".into(),
                ));
            }
            let removed = t.get(ctx, at);
            for i in at..n {
                t.set(ctx, i, t.get(ctx, i + 1))?;
            }
            if at <= n {
                t.set(ctx, n, Value::Nil)?;
            }
            Ok(vec![removed])
        }),
    )?;
    table.set(
        ctx,
        "concat",
        function(ctx, |ctx, args| {
            let Some(Value::Table(t)) = args.first().copied() else {
                return Err(lua_error(
                    ctx,
                    "bad argument #1 to 'concat' (table expected)".into(),
                ));
            };
            let sep = string_arg(&args, 1).unwrap_or_default();
            let first = args.get(2).and_then(|v| v.to_integer()).unwrap_or(1);
            let last = args
                .get(3)
                .and_then(|v| v.to_integer())
                .unwrap_or_else(|| t.length());
            let mut parts = Vec::new();
            for i in first..=last {
                let v = t.get(ctx, i);
                parts.push(string_arg(&[v], 0).ok_or_else(|| {
                    lua_error(
                        ctx,
                        format!("invalid value (at index {i}) in table for 'concat'"),
                    )
                })?);
            }
            Ok(vec![text(ctx, &parts.join(&sep))])
        }),
    )?;
    Ok(())
}

/// A string or number argument as text, as Lua coerces them.
fn string_arg(args: &[Value], i: usize) -> Option<String> {
    match args.get(i).copied()? {
        Value::String(s) => Some(s.to_str_lossy().into_owned()),
        Value::Integer(n) => Some(n.to_string()),
        Value::Number(n) => Some(number_text(n)),
        _ => None,
    }
}

/// `tostring` of a float: `1.0`, `0.5`, `1e+100`.
fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.1}")
    } else {
        format!("{n}")
    }
}

/// `string.format` for `%d %i %u %c %x %X %o %f %F %e %E %g %G %s %q %%` with
/// flags `-0+ #`, a width and a precision.
fn format(args: &[Value]) -> Result<String, String> {
    let template = string_arg(args, 0).ok_or("#1: string expected")?;
    let mut out = String::new();
    let mut next = 1;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }
        let mut flags = String::new();
        while let Some(&f) = chars.peek().filter(|f| "-0+ #".contains(**f)) {
            flags.push(f);
            chars.next();
        }
        let mut width = String::new();
        while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
            width.push(d);
            chars.next();
        }
        let mut precision = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut p = String::new();
            while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                p.push(d);
                chars.next();
            }
            precision = Some(p.parse::<usize>().unwrap_or(0));
        }
        let conversion = chars.next().ok_or("invalid conversion '%' to 'format'")?;
        let arg = args.get(next).copied().unwrap_or(Value::Nil);
        let what = format!("#{}", next + 1);
        next += 1;
        let integer = || -> Result<i64, String> {
            match arg {
                Value::Integer(i) => Ok(i),
                Value::Number(n) if n.fract() == 0.0 => Ok(n as i64),
                Value::Number(_) => Err(format!("{what}: number has no integer representation")),
                _ => Err(format!("{what}: number expected, got {}", arg.type_name())),
            }
        };
        let float = || -> Result<f64, String> {
            arg.to_number()
                .ok_or_else(|| format!("{what}: number expected, got {}", arg.type_name()))
        };
        let plus = flags.contains('+');
        let body = match conversion {
            'd' | 'i' | 'u' => {
                let n = integer()?;
                if plus && n >= 0 {
                    format!("+{n}")
                } else {
                    n.to_string()
                }
            }
            'c' => char::from_u32(integer()? as u32).unwrap_or('?').to_string(),
            'x' => format!("{:x}", integer()?),
            'X' => format!("{:X}", integer()?),
            'o' => format!("{:o}", integer()?),
            'f' | 'F' => {
                let n = float()?;
                let s = format!("{n:.*}", precision.unwrap_or(6));
                if plus && n >= 0.0 { format!("+{s}") } else { s }
            }
            'e' | 'E' => {
                let s = c_exponent(float()?, precision.unwrap_or(6));
                if conversion == 'E' {
                    s.to_uppercase()
                } else {
                    s
                }
            }
            'g' | 'G' => {
                let n = float()?;
                let p = precision.unwrap_or(6).max(1);
                let exp = if n == 0.0 {
                    0
                } else {
                    n.abs().log10().floor() as i32
                };
                let s = if exp < -4 || exp >= p as i32 {
                    c_exponent(n, p - 1)
                } else {
                    let s = format!("{n:.*}", (p as i32 - 1 - exp).max(0) as usize);
                    if s.contains('.') && !flags.contains('#') {
                        s.trim_end_matches('0').trim_end_matches('.').to_string()
                    } else {
                        s
                    }
                };
                if conversion == 'G' {
                    s.to_uppercase()
                } else {
                    s
                }
            }
            's' => {
                let s = match arg {
                    Value::String(s) => s.to_str_lossy().into_owned(),
                    Value::Number(n) => number_text(n),
                    Value::Nil => "nil".into(),
                    other => other.to_string(),
                };
                match precision {
                    Some(p) => s.chars().take(p).collect(),
                    None => s,
                }
            }
            'q' => format!(
                "{:?}",
                string_arg(&[arg], 0).ok_or(format!("{what}: string expected"))?
            ),
            other => return Err(format!("invalid conversion '%{other}' to 'format'")),
        };
        let width: usize = width.parse().unwrap_or(0);
        let pad = width.saturating_sub(body.chars().count());
        if flags.contains('-') {
            out.push_str(&body);
            out.extend(std::iter::repeat_n(' ', pad));
        } else if flags.contains('0') && !matches!(conversion, 's' | 'q' | 'c') {
            let (sign, digits) = match body.strip_prefix(['-', '+']) {
                Some(rest) => (&body[..1], rest),
                None => ("", body.as_str()),
            };
            out.push_str(sign);
            out.extend(std::iter::repeat_n('0', pad));
            out.push_str(digits);
        } else {
            out.extend(std::iter::repeat_n(' ', pad));
            out.push_str(&body);
        }
    }
    Ok(out)
}

/// C's `%e`: `1.500000e+02`.
fn c_exponent(n: f64, precision: usize) -> String {
    let s = format!("{n:.*e}", precision);
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    format!(
        "{mantissa}e{}{:02}",
        if exp < 0 { '-' } else { '+' },
        exp.abs()
    )
}
