#!/usr/bin/env python3
"""The Speedrunner's middle part (docs/companion-speedrun.md, N1-S5; game/companion_nether.rs), by segment.

Each run opens a fresh fixed-seed world (difficulty Normal), spawns a companion with the gear a speedrunner
realistically has at that point (iron pickaxe and sword, a shield unless --no-shield, a bucket, flint and
steel, food, throwaway blocks, gold boots), puts the owner in Watch mode, starts the segment and runs /tick sprint in
slices until the segment ends:

- nether:     at a lit Overworld portal -> into the Nether, find a fortress, 7 blaze rods.
- pearls:     in the Nether by the portal, with the rods -> 13 ender pearls, back through the portal.
- stronghold: in the Overworld with 7 rods and 13 pearls -> eyes, triangulate, travel, dig down, find the
              portal room, fill the frame.
- full:       at a lit Overworld portal -> a filled frame.

    python3 tools/test_companion_nether.py --client target/fast/horizon-client --offscreen \\
        --segment nether --perception all --runs 10 [--parallel 3]

Each run's result goes to .local/companion-nether/<segment>-<level>-<seed>.json (and its client log); the
tables (success rate, median time, deaths and their causes, how the failures ended) are printed and written
to .local/companion-nether/report-<segment>.md. Worlds live in temporary directories.
"""
import argparse
import concurrent.futures as futures
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

from test_companion import ROOT, command, wait_for

SEEDS = [20261020, 1, 42, 123456, 987654321, 7, 31337, 2024, 555, 8675309, 99, 4242, 13, 777, 100001]
SLICE = 1200
NAME = 'Runner'
SEGMENTS = {
    'nether': ('enter', 'blazes', 'overworld'),
    'pearls': ('pearls', 'home', 'nether'),
    'stronghold': ('eyes', 'fill', 'overworld'),
    'full': ('enter', 'fill', 'overworld'),
    # In the Nether with rods and pearls: back to the remembered portal and through it.
    'home': ('home', 'home', 'nether'),
}
# Game ticks a segment may take at most.
BUDGET = {'nether': 36000, 'pearls': 30000, 'home': 12000, 'stronghold': 60000, 'full': 120000}
LEVELS = {'all': 'all_knowing', 'all_knowing': 'all_knowing', 'fair': 'fair', 'scout': 'scout',
          'sharp': 'sharp'}


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


def runner(endpoint):
    for c in command(endpoint, 'companion list')['companions']:
        if c['name'] == NAME:
            return c
    return None


def block(endpoint, x, y, z):
    return command(endpoint, f'block {x} {y} {z}')['block']


def sprint(endpoint, ticks):
    command(endpoint, f'command tick sprint {ticks}')
    deadline = time.monotonic() + 600
    while time.monotonic() < deadline:
        if not command(endpoint, 'status').get('sprinting', False):
            return
        time.sleep(0.25)


def settle(endpoint, what, timeout=120):
    time.sleep(0.5)
    wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
             timeout=timeout, what=what)


def gear(endpoint, args, segment):
    # Gold boots keep piglins neutral, as every Nether entry in a real run does.
    items = ['iron_pickaxe', 'iron_sword', 'iron_shovel', 'bucket', 'flint_and_steel', 'cooked_beef 32',
             'cobblestone 64', 'netherrack 64', 'golden_boots']
    if not args.no_shield:
        items.append('shield')
    if args.bow:
        items += ['bow', 'arrow 32']
    if args.armour:
        items += ['iron_helmet', 'iron_chestplate', 'iron_leggings']
    if segment in ('pearls', 'home'):
        items.append('blaze_rod 7')
    if segment == 'home':
        items.append('ender_pearl 13')
    if segment == 'stronghold':
        items += ['blaze_rod 7', 'ender_pearl 13']
    for item in items:
        command(endpoint, f'command give {NAME} {item}')


def portal_by(endpoint):
    """A lit Nether portal three blocks in front of the companion, on a floor, in the open."""
    c = runner(endpoint)
    x, y, z = (math.floor(v) for v in c['position'])
    pz = z + 3
    command(endpoint, f'command fill {x - 3} {y} {z - 1} {x + 4} {y + 5} {z + 5} air')
    command(endpoint, f'command fill {x - 3} {y - 1} {z - 1} {x + 4} {y - 1} {z + 5} stone')
    command(endpoint, f'command fill {x - 1} {y - 1} {pz} {x + 2} {y + 3} {pz} obsidian')
    command(endpoint, f'command fill {x} {y} {pz} {x + 1} {y + 2} {pz} nether_portal[axis=x]')
    assert 'nether_portal' in block(endpoint, x, y, pz)
    return x, y, pz


def into_nether(endpoint, portal):
    """The companion walks into the portal, the watching owner's view comes along."""
    x, y, pz = portal
    command(endpoint, f'companion do {NAME} goto {x} {y} {pz}')
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        s = command(endpoint, 'status')
        c = runner(endpoint)
        if s['dimension'] == 'minecraft:the_nether' and s['pending_chunks'] == 0 and c \
                and c.get('dimension') == 'minecraft:the_nether':
            return
        time.sleep(0.3)
    raise AssertionError('the companion did not get through the portal')


def setup(endpoint, seed, args, segment):
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title screen')
    command(endpoint, f'open Nether{seed} {seed}')
    settle(endpoint, 'world loaded', timeout=240)
    command(endpoint, 'command difficulty normal')
    command(endpoint, 'time 1000')
    spawn = command(endpoint, 'status')['player']['world_spawn']
    command(endpoint, f'command tp @s {spawn[0] + 0.5} {spawn[1] + 1} {spawn[2] + 0.5}')
    settle(endpoint, 'owner at the spawn')
    command(endpoint, f'companion spawn {NAME}')
    wait_for(endpoint, 'companion list', lambda v: runner(endpoint) and not runner(endpoint)['dead'],
             what='the runner')
    command(endpoint, f'companion do {NAME} stay')
    time.sleep(0.5)
    gear(endpoint, args, segment)
    command(endpoint, f'companion watch {NAME}')
    _, _, where = SEGMENTS[segment]
    if segment in ('nether', 'full'):
        portal_by(endpoint)
    elif where == 'nether':
        command(endpoint, 'command difficulty peaceful')
        into_nether(endpoint, portal_by(endpoint))
        if segment == 'home':
            # Off a way from the portal it came through, to walk back.
            c = runner(endpoint)
            x, y, z = (math.floor(v) for v in c['position'])
            command(endpoint, f'command fill {x + 18} {y} {z - 1} {x + 22} {y + 2} {z + 1} air')
            command(endpoint, f'command fill {x + 18} {y - 1} {z - 1} {x + 22} {y - 1} {z + 1} netherrack')
            command(endpoint, f'command tp {NAME} {x + 20.5} {y} {z + 0.5}')
            time.sleep(0.5)
        command(endpoint, 'command difficulty normal')
    time.sleep(0.5)


def one_run(endpoint, seed, args, segment, out):
    setup(endpoint, seed, args, segment)
    first, until, _ = SEGMENTS[segment]
    level = LEVELS[args.perception]
    words = f'nether {first} until {until} {level}'
    if args.rods:
        words += f' rods {args.rods}'
    if args.pearls:
        words += f' pearls {args.pearls}'
    command(endpoint, f'companion do {NAME} {words}')
    started = time.monotonic()
    result = {'seed': seed, 'segment': segment, 'level': level, 'shield': not args.no_shield}
    done = 0
    nether = {}
    still, dumped = [], set()
    hurts = []
    command(endpoint, 'hurtlog')
    while done < BUDGET[segment] and time.monotonic() - started < args.wall:
        sprint(endpoint, args.slice)
        done += args.slice
        # Every hurt it took (the game's hurtlog keeps the last 64 per slice).
        for p in command(endpoint, 'hurtlog')['players']:
            if p['name'] == NAME:
                hurts += [dict(h, slice=done) for h in p['hurts']]
        c = runner(endpoint)
        if c is None:
            result['error'] = 'the runner is gone'
            break
        nether = c.get('nether') or {}
        sample = {k: nether.get(k) for k in ('leg', 'step', 'note', 'ticks', 'rods', 'pearls', 'eyes',
                                             'outcome', 'pit', 'waypoint', 'cruise')}
        dig = c.get('digging')
        sample.update({'walk': c.get('walk'), 'digging': dig, 'mining': c.get('mining'),
                       'dig_block': block(endpoint, *dig) if dig else None,
                       'held': c.get('held'), 'keys': c.get('keys'),
                       'position': c.get('position'), 'dimension': c.get('dimension'),
                       'health': c.get('health'), 'food': c.get('food'),
                       'wall_s': round(time.monotonic() - started, 1)})
        # Stalled (600 ticks without moving): what is around it, once per spot.
        pos = c.get('position')
        if pos:
            still.append((done, pos))
            while still and done - still[0][0] > 600:
                still.pop(0)
            spot = tuple(math.floor(v) for v in pos)
            if (len(still) > 1 and done - still[0][0] >= 600 - args.slice and spot not in dumped
                    and max(abs(a - b) for (_, p0) in still for a, b in zip(p0, pos)) < 0.3):
                dumped.add(spot)
                x, y, z = spot
                sample['stall'] = {f'{dx},{dy},{dz}': block(endpoint, x + dx, y + dy, z + dz).replace('minecraft:', '')
                                   for dy in range(-2, 4) for dx in range(-2, 3) for dz in range(-2, 3)}
        out.write(json.dumps(sample) + '\n')
        out.flush()
        if nether.get('outcome'):
            break
    hud = command(endpoint, 'companion watch status').get('hud')
    reports = [r['text'] for r in command(endpoint, 'companion reports')['reports']]
    result.update({
        'outcome': nether.get('outcome') or f'{nether.get("leg")}: out of the harness budget',
        'success': nether.get('outcome') == 'done',
        'ticks': nether.get('ticks', done),
        'wall_s': round(time.monotonic() - started, 1),
        'splits': {s['name']: s['ticks'] for s in nether.get('splits', [])},
        'deaths': nether.get('deaths', []),
        'min_health': nether.get('min_health'),
        'rods': nether.get('rods'), 'pearls': nether.get('pearls'), 'eyes': nether.get('eyes'),
        'filled': nether.get('filled'), 'throws': nether.get('throws'), 'stronghold': nether.get('stronghold'),
        'hud': hud,
        'taken_by_source': {k: round(sum(h['taken'] for h in hurts if h['source'] == k), 1)
                            for k in sorted({h['source'] for h in hurts})},
        'fireballs_blocked': [sum(1 for h in hurts if h['source'] == 'minecraft:fireball' and h['blocked'] > 0),
                              sum(1 for h in hurts if h['source'] == 'minecraft:fireball')],
        'hurts_last': [(h['t'], h['source'].replace('minecraft:', ''), h['blocked'] > 0, round(h['taken'], 1),
                        round(h['health'], 1)) for h in hurts[-30:]],
        'milestones': [r for r in reports if ':' in r and not r.startswith('hurt')][-30:],
    })
    if segment == 'stronghold' and nether.get('stronghold'):
        # How near the triangulation came: /locate after the fact.
        try:
            located = command(endpoint, 'command locate structure minecraft:stronghold')
            result['locate'] = located.get('chat')
        except AssertionError:
            pass
    command(endpoint, 'companion watch off')
    command(endpoint, 'quit')
    return result


def run_seed(args, segment, seed):
    results = ROOT / '.local/companion-nether'
    results.mkdir(parents=True, exist_ok=True)
    name = f'{segment}-{LEVELS[args.perception]}{args.tag}-{seed}'
    with tempfile.TemporaryDirectory(prefix='horizon-nether-') as directory:
        home = Path(directory) / 'game'
        home.mkdir()
        endpoint = home / 'control.sock'
        with (home / 'client.log').open('w+') as log, (results / f'{name}.samples').open('w') as out:
            process = start(args.client, args.resources, home, endpoint, log, args.offscreen,
                            timeout=args.wall + 600)
            try:
                result = one_run(endpoint, seed, args, segment, out)
                process.wait(timeout=60)
            except Exception as e:  # a harness or game failure, kept with the log's end
                log.seek(0)
                tail = log.read().splitlines()[-40:]
                result = {'seed': seed, 'segment': segment, 'level': LEVELS[args.perception],
                          'error': repr(e), 'log': tail, 'success': False, 'outcome': f'harness: {e!r}'[:120],
                          'splits': {}, 'deaths': []}
            finally:
                stop(process)
            log.seek(0)
            lines = [l for l in log.read().splitlines() if '[companion]' in l or 'panick' in l]
            (results / f'{name}.log').write_text('\n'.join(lines[-4000:]))
    (results / f'{name}.json').write_text(json.dumps(result, indent=1))
    print(json.dumps({k: result.get(k) for k in ('seed', 'outcome', 'ticks', 'wall_s', 'splits', 'deaths')}),
          flush=True)
    return result


def mmss(ticks):
    return f'{int(ticks // 1200)}:{int(ticks % 1200 / 20):02d}' if ticks is not None else '-'


def table(segment, level, results):
    ok = [r for r in results if r.get('success')]
    times = [r['ticks'] for r in ok]
    deaths = [d for r in results for d in r.get('deaths', [])]
    lines = [f'### {segment}, {level} ({len(results)} runs)', '',
             '| Success | Median time (game m:s) | p90 | Deaths |', '| --- | --- | --- | --- |',
             f'| {len(ok)}/{len(results)} | {mmss(statistics.median(times)) if times else "-"} | '
             f'{mmss(sorted(times)[int(len(times) * 0.9) - 1 if len(times) > 1 else 0]) if times else "-"} | '
             f'{len(deaths)} |', '']
    splits = sorted({s for r in results for s in r.get('splits', {})},
                    key=lambda s: statistics.median([r['splits'][s] for r in results if s in r['splits']]))
    if splits:
        lines += ['| Split | Reached | Median (game m:s) |', '| --- | --- | --- |']
        for s in splits:
            t = [r['splits'][s] for r in results if s in r.get('splits', {})]
            lines.append(f'| {s} | {len(t)}/{len(results)} | {mmss(statistics.median(t))} |')
        lines.append('')
    fails = {}
    for r in results:
        if not r.get('success'):
            fails[r.get('outcome')] = fails.get(r.get('outcome'), 0) + 1
    if fails:
        lines += ['| Failure | Runs |', '| --- | --- |']
        lines += [f'| {k} | {v} |' for k, v in sorted(fails.items(), key=lambda kv: -kv[1])]
        lines.append('')
    if deaths:
        causes = {}
        for d in deaths:
            causes[d] = causes.get(d, 0) + 1
        lines += ['| Death cause (leg) | Count |', '| --- | --- |']
        lines += [f'| {k} | {v} |' for k, v in sorted(causes.items(), key=lambda kv: -kv[1])]
        lines.append('')
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--segment', choices=sorted(SEGMENTS), default='nether')
    parser.add_argument('--perception', choices=sorted(LEVELS), default='all')
    parser.add_argument('--runs', type=int, default=10)
    parser.add_argument('--seed', type=int, action='append', help='these seeds instead')
    parser.add_argument('--parallel', type=int, default=1)
    parser.add_argument('--wall', type=int, default=900, help='wall seconds a run may take')
    parser.add_argument('--no-shield', action='store_true')
    parser.add_argument('--slice', type=int, default=SLICE, help='ticks per sample (smaller to trace a fight)')
    parser.add_argument('--bow', action='store_true', help='give a bow and arrows too')
    parser.add_argument('--armour', action='store_true',
                        help='iron helmet, chestplate and leggings too (the gold boots stay for the piglins)')
    parser.add_argument('--tag', default='', help='a suffix for the result files')
    parser.add_argument('--rods', type=int)
    parser.add_argument('--pearls', type=int)
    args = parser.parse_args()
    args.client, args.resources = args.client.resolve(), args.resources.resolve()
    seeds = args.seed or SEEDS[:args.runs]
    with futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
        results = list(pool.map(lambda s: run_seed(args, args.segment, s), seeds))
    report = table(args.segment, LEVELS[args.perception], results)
    print(report)
    out = ROOT / '.local/companion-nether' / f'report-{args.segment}-{LEVELS[args.perception]}{args.tag}.md'
    out.write_text(report + '\n')


if __name__ == '__main__':
    main()
