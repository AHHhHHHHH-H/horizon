#!/usr/bin/env python3
"""Speedrunner competence harness (docs/companion-speedrun.md, "Harness"):
a companion runs the overworld route alone in fixed-seed worlds, at one
perception level, under /tick sprint, while its owner watches as a
spectator camera (Watch mode). Each run records the game time of every
milestone (wooden pickaxe, stone tools, food, iron, iron pickaxe, buckets,
ten obsidian, portal lit, first night survived), the stages that ran out of
time and why, deaths, and what the companion's tick cost.

    python3 tools/test_speedrun.py --client target/fast/horizon-client --offscreen \\
        --perception all --seeds 5 [--parallel 3] [--ticks 72000]

Results go to .local/speedrun/<level>-<seed>.json and a table is printed.
Worlds live in temporary directories.
"""
import argparse
import concurrent.futures as futures
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

from test_companion import ROOT, command, companion, wait_for

SEEDS = [20261020, 1, 42, 123456, 987654321, 7, 31337, 2024, 555, 8675309]
MILESTONES = ['wooden_pickaxe', 'stone_tools', 'food', 'iron_10', 'iron_pickaxe', 'buckets',
              'obsidian_10', 'portal_lit', 'first_night']
SLICE = 1200


def start(client, resources, home, endpoint, log, offscreen, timeout):
    extra = ['--offscreen'] if offscreen else []
    return subprocess.Popen([
        str(client), '--resources', str(resources), '--render-distance', '6',
        '--control-socket', str(endpoint), '--timeout-seconds', str(timeout), *extra,
    ], cwd=home, env=dict(os.environ, HORIZON_HOME=str(home), HORIZON_NO_AUDIO='1',
                         HORIZON_MENU_STYLE='classic', HORIZON_PLAYER_NAME='Owner'),
        stdout=log, stderr=subprocess.STDOUT)


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)


def one_run(endpoint, seed, level, budget, out):
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title screen')
    command(endpoint, f'open Speedrun{seed} {seed}')
    wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
             timeout=240, what='world loaded')
    command(endpoint, 'command difficulty easy')
    command(endpoint, 'companion spawn Runner')
    wait_for(endpoint, 'companion list', lambda v: v['companions'] and not v['companions'][0]['dead'],
             what='runner')
    command(endpoint, 'companion watch Runner')
    command(endpoint, f'companion do Runner run {level}')
    started = time.monotonic()
    log = {'seed': seed, 'level': level, 'ticks': 0, 'samples': []}
    done = 0
    last = None
    while done < budget:
        command(endpoint, f'command tick sprint {SLICE}')
        # The sprint ends by itself; wait until the clock is back to normal.
        deadline = time.monotonic() + 600
        while time.monotonic() < deadline:
            if not command(endpoint, 'status').get('sprinting', False):
                break
            time.sleep(0.5)
        done += SLICE
        c = companion(endpoint)
        if c is None:
            break
        run = c.get('run') or {}
        last = run
        log['samples'].append({
            'ticks': run.get('ticks'), 'stage': run.get('stage'), 'note': run.get('note'),
            'skill': c.get('skill'), 'task': c.get('task'),
            'position': c.get('position'), 'health': c.get('health'), 'food': c.get('food'),
            'wall_s': round(time.monotonic() - started, 1),
        })
        out.write(json.dumps(log['samples'][-1]) + '\n')
        out.flush()
        splits = {s['name'] for s in run.get('splits', [])}
        if 'portal_lit' in splits and 'first_night' in splits:
            break
        if 'portal_lit' in splits and done > 30000:
            break
    cost = command(endpoint, 'companion cost').get('cost', {})
    final = companion(endpoint) or {}
    log.update({
        'ticks': (last or {}).get('ticks', done),
        'wall_s': round(time.monotonic() - started, 1),
        'splits': {s['name']: s['ticks'] for s in (last or {}).get('splits', [])},
        'stage': (last or {}).get('stage'),
        'failed': (last or {}).get('failed', []),
        'failure': (last or {}).get('failure'),
        'deaths': (last or {}).get('deaths', 0),
        'placed': (last or {}).get('placed'),
        'dug': (last or {}).get('dug'),
        'plans': (last or {}).get('plans'),
        'plan_max_ms': (last or {}).get('plan_max_ms'),
        'known': (last or {}).get('known'),
        'inventory': final.get('inventory'),
        'cost': cost,
    })
    command(endpoint, 'companion watch off')
    command(endpoint, 'quit')
    return log


def run_seed(args, seed):
    results = ROOT / '.local/speedrun'
    results.mkdir(parents=True, exist_ok=True)
    name = f'{args.perception}-{seed}'
    with tempfile.TemporaryDirectory(prefix='horizon-speedrun-') as directory:
        home = Path(directory) / 'game'
        home.mkdir()
        endpoint = home / 'control.sock'
        with (home / 'client.log').open('w+') as log, (results / f'{name}.samples').open('w') as out:
            process = start(args.client, args.resources, home, endpoint, log, args.offscreen,
                            timeout=args.ticks // 10 + 1200)
            try:
                result = one_run(endpoint, seed, args.perception, args.ticks, out)
                process.wait(timeout=60)
            except Exception as e:
                log.seek(0)
                tail = log.read().splitlines()[-60:]
                result = {'seed': seed, 'level': args.perception, 'error': repr(e), 'log': tail,
                          'splits': {}}
            finally:
                stop(process)
            log.seek(0)
            (results / f'{name}.log').write_text(log.read())
    (results / f'{name}.json').write_text(json.dumps(result, indent=1))
    return result


def table(level, results):
    rows = [f'| Milestone ({level}) | Success | Median time (game min:s) |', '| --- | --- | --- |']
    for m in MILESTONES:
        times = [r['splits'][m] for r in results if m in r.get('splits', {})]
        median = statistics.median(times) if times else None
        shown = f'{int(median // 1200)}:{int(median % 1200 / 20):02d}' if median is not None else '-'
        rows.append(f'| {m} | {len(times)}/{len(results)} | {shown} |')
    return '\n'.join(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--perception', default='all_knowing')
    parser.add_argument('--seeds', type=int, default=5)
    parser.add_argument('--seed', type=int, action='append', help='these seeds instead')
    parser.add_argument('--ticks', type=int, default=72000, help='game ticks per run at most')
    parser.add_argument('--parallel', type=int, default=1)
    args = parser.parse_args()
    args.client, args.resources = args.client.resolve(), args.resources.resolve()
    seeds = args.seed or SEEDS[:args.seeds]
    with futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
        results = list(pool.map(lambda s: run_seed(args, s), seeds))
    for r in results:
        print(json.dumps({k: r.get(k) for k in ('seed', 'stage', 'ticks', 'wall_s', 'splits', 'failed',
                                                 'failure', 'deaths', 'error')}))
    print(table(args.perception, results))


if __name__ == '__main__':
    main()
