#!/usr/bin/env python3
"""The Speedrunner's lava-cast nether portal (docs/companion-speedrun.md,
"The route"): on a stone platform built with commands in a fixed-seed world,
with a 5x5 lava pool sunk in it and a water source, a companion that carries
the iron kit (two empty buckets, flint and steel, cobblestone) starts its run
at the portal stage. It fills a bucket with water, builds the mould beside
the pool, casts the ten obsidian blocks one by one (lava poured against the
back wall, water beside or above it, the water taken back), and lights the
portal. Under /tick sprint.

    python3 tools/test_companion_portal.py --client target/fast/horizon-client --offscreen [--level all_knowing]
"""
import argparse
import json
import math
import os
from pathlib import Path
import tempfile
import time

from test_companion import ROOT, SEED, command, companion, wait_for
from test_speedrun import start, stop

X, Y, Z = 300, 150, 300


def say(endpoint, line):
    return command(endpoint, f'command {line}').get('chat', [])


def build(endpoint):
    say(endpoint, f'fill {X - 14} {Y + 1} {Z - 14} {X + 14} {Y + 16} {Z + 14} air')
    say(endpoint, f'fill {X - 14} {Y - 6} {Z - 14} {X + 14} {Y} {Z + 14} stone')
    # The lava pool, flush with the floor, east of the centre.
    say(endpoint, f'fill {X + 4} {Y} {Z - 2} {X + 8} {Y} {Z + 2} lava')
    # A water source in a hole, west.
    say(endpoint, f'setblock {X - 6} {Y} {Z} water')


def scenario(endpoint, level, budget):
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title screen')
    command(endpoint, f'open CompanionPortal {SEED}')
    wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
             timeout=240, what='world loaded')
    for rule in ('advance_time', 'spawn_mobs'):
        say(endpoint, f'gamerule {rule} false')
    say(endpoint, 'time set 1000')
    say(endpoint, 'gamemode creative')
    say(endpoint, f'tp @s {X + 0.5} {Y + 1} {Z + 0.5}')
    wait_for(endpoint, 'status', lambda v: v['player'] and v['pending_chunks'] == 0, timeout=60,
             what='owner at the platform')
    # Under load the fill may land before its chunks are ready: again
    # until the platform, the pool and the water are there.
    def built(v):
        return all(command(endpoint, f'block {x} {y} {z}')['block'].startswith(b)
                   for x, y, z, b in ((X, Y, Z, 'minecraft:stone'), (X + 6, Y, Z, 'minecraft:lava'),
                                      (X - 6, Y, Z, 'minecraft:water'), (X - 13, Y - 6, Z + 13, 'minecraft:stone')))
    for _ in range(10):
        build(endpoint)
        if built(None):
            break
        time.sleep(1)
    assert built(None), 'the platform was not built'
    # The owner may have dropped through before the platform was there.
    say(endpoint, f'tp @s {X + 0.5} {Y + 1} {Z + 0.5}')
    name = command(endpoint, 'companion spawn Runner')['name']
    wait_for(endpoint, 'companion list', lambda v: v['companions'] and not v['companions'][0]['dead'],
             what='runner')
    # Staying put: following, it would be teleported back to the owner.
    command(endpoint, f'companion do {name} stay')
    say(endpoint, f'clear {name}')
    for item in ('bucket 2', 'flint_and_steel', 'cobblestone 64', 'iron_pickaxe', 'cooked_beef 8'):
        say(endpoint, f'give {name} {item}')
    # Standing on the platform (not hung in a chunk still loading, nor
    # fallen through one): put there again until it is.
    def on_platform(v):
        c = v['companions'] and v['companions'][0]
        return c and c.get('on_ground') and abs(c['position'][1] - (Y + 1)) < 0.1 \
            and math.dist(c['position'][::2], (X + 0.5, Z + 0.5)) < 1.5
    for attempt in range(6):
        say(endpoint, f'tp {name} {X + 0.5} {Y + 1} {Z + 0.5}')
        try:
            wait_for(endpoint, 'companion list', on_platform, timeout=10, what='runner on the platform')
            break
        except AssertionError:
            if attempt == 5:
                raise
    # A moment to look around (the water, the pool) before it starts: the
    # platform floats high, and a search off its edge is a fall.
    command(endpoint, f'companion do {name} stay')
    command(endpoint, 'command tick sprint 60')
    while command(endpoint, 'status').get('sprinting'):
        time.sleep(0.1)
    command(endpoint, f'companion watch {name}')
    command(endpoint, f'companion do {name} run {level} from portal')
    done, lit = 0, None
    same, last = 0, None
    while done < budget:
        command(endpoint, 'command tick sprint 200')
        while command(endpoint, 'status').get('sprinting'):
            time.sleep(0.1)
        done += 200
        c = companion(endpoint)
        run = c.get('run') or {}
        splits = {s['name']: s['ticks'] for s in run.get('splits', [])}
        print(done, run.get('stage'), '|', run.get('note'), '|', c.get('inventory'), '|', [round(v, 2) for v in c['position']], (c.get('walk') or {}).get('next'), (c.get('walk') or {}).get('goal'))
        # The same thing for 1200 ticks: what the mould looks like.
        same = same + 1 if run.get('note') == last else 0
        last = run.get('note')
        if same == 6:
            print('stuck: walk', c.get('walk'))
            dump(endpoint, run.get('cast') or {})
        if 'portal_lit' in splits:
            lit = splits['portal_lit']
            break
    cast = (companion(endpoint).get('run') or {}).get('cast') or {}
    print('cast', cast)
    dump(endpoint, cast)
    command(endpoint, 'companion watch off')
    command(endpoint, 'quit')
    return lit


def dump(endpoint, cast):
    """The mould's three planes (back wall, frame, front), top down."""
    if cast.get('base'):
        bx, by, bz = cast['base']
        front = cast['front']
        short = {'obsidian': 'O', 'cobblestone': 'c', 'air': '.', 'water': 'w', 'lava': 'L', 'stone': 's',
                 'nether_portal': 'P', 'fire': 'f'}
        for dz, label in ((-front, 'back'), (0, 'frame'), (front, 'front'), (2 * front, 'front 2')):
            print(label)
            for y in range(by + 5, by - 2, -1):
                row = ''
                for x in range(bx - 1, bx + 5):
                    n = command(endpoint, f'block {x} {y} {bz + dz}')['block'].split(':')[1].split('[')[0]
                    row += short.get(n, '?')
                print(f'  {y}: {row}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--level', default='all_knowing')
    parser.add_argument('--ticks', type=int, default=12000)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='horizon-companion-portal-') as directory:
        home = Path(directory) / 'game'
        home.mkdir()
        endpoint = home / 'control.sock'
        with (home / 'client.log').open('w+') as log:
            process = start(args.client.resolve(), args.resources.resolve(), home, endpoint, log,
                            args.offscreen, 1800)
            try:
                lit = scenario(endpoint, args.level, args.ticks)
            finally:
                stop(process)
                log.seek(0)
                text = log.read()
                if os.environ.get('HORIZON_PORTAL_LOG'):
                    Path(os.environ['HORIZON_PORTAL_LOG']).write_text(text)
                lines = [l for l in text.splitlines()
                         if '[companion]' in l and "o'clock" not in l and 'equipped' not in l and '] slow ' not in l]
                print('\n'.join(lines[-80:]))
    assert lit is not None, 'the portal was not lit'
    print(json.dumps({'portal_lit_ticks': lit}))
    print('Companion portal passed: water, mould, ten cast obsidian, lit')


if __name__ == '__main__':
    main()
