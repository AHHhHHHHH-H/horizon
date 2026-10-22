#!/usr/bin/env python3
"""Machine harness for code-mod ports (docs/research/code-mods-plan.md): the same scenarios on the
Java mod in the dev-only NeoForge oracle and on its Horizon port, compared tick by tick.

    mod_machine_harness.py --jars DIR [--deps DIR] --hmod PORT.hmod --scenarios S.json --work DIR
        [--prepared .local/horizon-client/26.3] [--oracle TRACE.json]

A scenario places a block, fills slots, runs N ticks (with slot changes at given ticks) and
records after every tick the slots, the listed block-state properties and the listed integer
fields of the saved block entity (the Java tag's names; the port saves the same names as JSON).
Writes WORK/oracle-trace.json, WORK/horizon-trace.json and WORK/result.json, and exits 1 on a
difference. The jars and the port stay in the work folder: nothing here holds mod code.
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def compare(oracle, horizon):
    """Per scenario: ticks compared, ticks equal, the first differences."""
    out = []
    by_name = {s['name']: s['trace'] for s in horizon}
    for scenario in oracle:
        name, want = scenario['name'], scenario['trace']
        got = by_name.get(name, [])
        equal, diffs = 0, []
        for tick, (w, g) in enumerate(zip(want, got)):
            if w == g:
                equal += 1
            elif len(diffs) < 5:
                keys = sorted({*w, *g})
                diffs.append({'tick': tick, **{k: {'java': w.get(k), 'horizon': g.get(k)}
                                               for k in keys if w.get(k) != g.get(k)}})
        if len(want) != len(got):
            diffs.append({'ticks': {'java': len(want), 'horizon': len(got)}})
        out.append({'name': name, 'ticks': len(want), 'equal': equal, 'differences': diffs})
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--jars', help='the Java mod (and its namesakes); not needed with --oracle')
    p.add_argument('--deps')
    p.add_argument('--hmod', required=True)
    p.add_argument('--scenarios', required=True)
    p.add_argument('--work', required=True)
    p.add_argument('--prepared', default=str(ROOT / '.local' / 'horizon-client' / '26.3'))
    p.add_argument('--oracle', help='an oracle trace already written')
    args = p.parse_args()
    work = Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    oracle_path = Path(args.oracle) if args.oracle else work / 'oracle-trace.json'
    if not args.oracle:
        if not args.jars:
            sys.exit('--jars or --oracle')
        command = [sys.executable, str(ROOT / 'tools' / 'neoforge_oracle.py'), 'machines', '--jars', args.jars,
                   '--scenarios', args.scenarios, '--out', str(oracle_path)]
        if args.deps:
            command += ['--deps', args.deps]
        subprocess.run(command, check=True)
    horizon_path = work / 'horizon-trace.json'
    subprocess.run(['cargo', 'run', '--release', '-q', '-p', 'horizon-client', '--example', 'mod_machine_probe',
                    '--', args.prepared, args.hmod, args.scenarios, str(horizon_path)], cwd=ROOT, check=True)
    result = compare(json.loads(oracle_path.read_text()), json.loads(horizon_path.read_text()))
    (work / 'result.json').write_text(json.dumps(result, indent=1))
    failed = False
    for r in result:
        ok = not r['differences']
        failed |= not ok
        print(f"{'PASS' if ok else 'FAIL'} {r['name']}: {r['equal']}/{r['ticks']} ticks equal")
        for d in r['differences'][:3]:
            print('   ', json.dumps(d))
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
