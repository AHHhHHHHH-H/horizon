//! TEMPORARY probe (#270), not committed.
use std::collections::HashMap;
use std::sync::Mutex;
pub static SETS: Mutex<Option<HashMap<(u16, u16), u64>>> = Mutex::new(None);
pub static SRC: Mutex<[u64; 6]> = Mutex::new([0; 6]);
pub fn set(old: u16, new: u16) {
    *SETS.lock().unwrap().get_or_insert_with(HashMap::new).entry((old, new)).or_default() += 1;
}
pub fn src(i: usize, n: u64) {
    SRC.lock().unwrap()[i] += n;
}
pub fn dump(t: f64, meshed: u64) {
    let sets = SETS.lock().unwrap().take().unwrap_or_default();
    let src = std::mem::take(&mut *SRC.lock().unwrap());
    let mut v: Vec<_> = sets.into_iter().collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.1));
    let total: u64 = v.iter().map(|e| e.1).sum();
    eprintln!("PROBE t={t:.1} meshed={meshed} sets={total} src(set,light,lit,other,lightpos,rlit)={src:?} top={:?}", &v[..v.len().min(8)]);
}
