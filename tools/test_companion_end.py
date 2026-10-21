#!/usr/bin/env python3
"""The companion's End route, end to end (docs/companion-speedrun.md, "The
End"): a companion stands in a stronghold-like portal room with some frames
filled, carrying what a speedrunner brings (iron armour or none, an iron
sword and pickaxe, a bow and a stack of arrows, three stacks of blocks,
food, the missing eyes, beds for the bed variant). Its owner watches (Watch
mode, a spectator camera), the game runs under `/tick sprint` at difficulty
Normal, and the companion is told `companion do end <level> [beds]`: it
fills the frames, jumps in, gets off the arrival platform, finds and
destroys the crystals, kills the dragon and leaves by the exit portal,
alone.

Each run is a fresh world; runs go across seeds and perception levels, a
few clients at a time. The table at the end gives, per level: successes,
the time to kill (game time from entering the End), deaths and their causes,
and where failed runs stopped.

    python3 tools/test_companion_end.py --client target/fast/horizon-client \\
        --offscreen --seeds 1 2 3 4 5 --levels all fair [--beds] [--jobs 3]

Linux needs a display (xvfb-run -a); macOS can pass --offscreen. Worlds live
in temporary directories; --keep keeps each run's client log and status trail.
"""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_companion import command, wait_for, stop  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
NAME = 'Ari'
# Where the portal room is built: high in the sky, away from the terrain
# and the overworld's mobs.
ROOM_Y = 200


def start(client, resources, home, endpoint, log, offscreen, timeout):
    extra = ['--offscreen'] if offscreen else []
    return subprocess.Popen([
        str(client), '--resources', str(resources), '--render-distance', '10',
        '--control-socket', str(endpoint), '--timeout-seconds', str(timeout), *extra,
    ], cwd=home, env=dict(os.environ, HORIZON_HOME=str(home), HORIZON_NO_AUDIO='1',
                         HORIZON_MENU_STYLE='classic', HORIZON_PLAYER_NAME='Owner'),
        stdout=log, stderr=subprocess.STDOUT)


def body(endpoint):
    listed = command(endpoint, 'companion list')['companions']
    return next((c for c in listed if c.get('name') == NAME), None)


def portal_room(endpoint, rng):
    """A floor, a ring of 12 frames facing in around a 3x3 opening, some
    with an eye (a stronghold's frames have one 10% of the time; here 0 to
    6 so that the companion has some to fill). Returns the eyes missing."""
    y = ROOM_Y
    command(endpoint, f'command fill -10 {y - 1} -6 10 {y - 1} 14 stone')
    command(endpoint, f'command fill -10 {y} -6 10 {y + 6} 14 air')
    cx, cz = 0, 6
    frames = []
    for d in (-1, 0, 1):
        frames.append((cx + d, cz - 2, 'south'))
        frames.append((cx + d, cz + 2, 'north'))
        frames.append((cx - 2, cz + d, 'east'))
        frames.append((cx + 2, cz + d, 'west'))
    filled = set(rng.sample(range(12), rng.randint(0, 6)))
    for i, (x, z, facing) in enumerate(frames):
        eye = 'true' if i in filled else 'false'
        command(endpoint, f'command setblock {x} {y} {z} minecraft:end_portal_frame[facing={facing},eye={eye}]')
    # Lava under the opening, as in a stronghold (it falls through the portal, not in).
    command(endpoint, f'command fill {cx - 1} {y - 1} {cz - 1} {cx + 1} {y - 1} {cz + 1} lava')
    return 12 - len(filled)


def gear(endpoint, args, missing, rng):
    items = ['iron_sword', 'iron_pickaxe', 'bow', f'arrow {args.arrows}', 'cobblestone 64',
             'cobblestone 64', 'cobblestone 64', 'cooked_beef 24', 'bread 16',
             f'ender_eye {missing}']
    if args.armour == 'iron':
        items += ['iron_helmet', 'iron_chestplate', 'iron_leggings', 'iron_boots']
    if args.beds:
        items.append('white_bed 8')
    if args.pumpkin:
        items.append('carved_pumpkin')
    for item in items:
        command(endpoint, f'command give {NAME} {item}')


def one_run(args, seed, level):
    """One world: returns a dict of what happened."""
    rng = random.Random(seed * 31 + len(level))
    out = {'seed': seed, 'level': level, 'beds': args.beds, 'armour': args.armour,
           'pumpkin': args.pumpkin}
    with tempfile.TemporaryDirectory(prefix='horizon-end-') as directory:
        home = Path(directory) / 'game'
        home.mkdir()
        endpoint = home / 'control.sock'
        trail = []
        with (home / 'client.log').open('w+') as log:
            process = start(args.client, args.resources, home, endpoint, log, args.offscreen,
                            args.timeout + 600)
            started = time.monotonic()
            try:
                wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', timeout=120)
                command(endpoint, f'open End{seed} {seed}')
                wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
                         timeout=300, what='world loaded')
                command(endpoint, 'time 1000')
                for rule in ('advance_time', 'spawn_patrols', 'spawn_phantoms', 'advance_weather'):
                    command(endpoint, f'command gamerule {rule} false')
                command(endpoint, 'command difficulty normal')
                command(endpoint, 'command gamemode creative')
                command(endpoint, f'command tp @s 0.5 {ROOM_Y + 1} -3.5')
                time.sleep(1)
                missing = portal_room(endpoint, rng)
                command(endpoint, f'command tp @s 0.5 {ROOM_Y} -3.5')
                command(endpoint, f'companion spawn {NAME}')
                # Into the room, checked: under load the body may not be
                # there yet, and it would start in the ground at spawn.
                for _ in range(20):
                    time.sleep(0.5)
                    c = body(endpoint)
                    if c and abs(c.get('position', [0, 0, 0])[1] - ROOM_Y) < 1.5:
                        break
                    if c:
                        command(endpoint, f'command tp {NAME} 0.5 {ROOM_Y} 0.5 0 0')
                time.sleep(0.5)
                # Its post where it stands now (a Stay before the /tp walks it back).
                command(endpoint, f'companion do {NAME} stay')
                gear(endpoint, args, missing, rng)
                time.sleep(2)
                command(endpoint, f'companion watch {NAME}')
                command(endpoint, 'render-distance 10')
                command(endpoint, 'command tick sprint 1000000')
                # Still in the room before the order (see above): where it
                # starts goes in the results.
                c = body(endpoint)
                if c and abs(c.get('position', [0, 0, 0])[1] - ROOM_Y) >= 1.5:
                    command(endpoint, f'command tp {NAME} 0.5 {ROOM_Y} 0.5 0 0')
                    time.sleep(0.5)
                    c = body(endpoint)
                out['start'] = [round(v, 1) for v in (c or {}).get('position', [0, 0, 0])]
                order = f'companion do {NAME} end {level}' + (' beds' if args.beds else '')
                command(endpoint, order)
                out['eyes_missing'] = missing
                last = None
                while time.monotonic() - started < args.timeout:
                    time.sleep(2)
                    try:
                        c = body(endpoint)
                        fight = command(endpoint, 'status').get('fight') or {}
                    except (ConnectionRefusedError, FileNotFoundError, AssertionError, OSError):
                        if process.poll() is not None:
                            out['failure'] = f'client exited ({process.returncode})'
                            break
                        continue
                    end = (c or {}).get('end') or {}
                    dragon = fight.get('dragon') or {}
                    row = {k: end.get(k) for k in ('phase', 'note', 'ticks', 'destroyed', 'known',
                                                    'dragon_health', 'deaths', 'danger')}
                    row['health'] = (c or {}).get('health')
                    row['food'] = (c or {}).get('food')
                    row['dim'] = (c or {}).get('dimension')
                    row['pos'] = [round(v, 1) for v in (c or {}).get('position', [0, 0, 0])]
                    row['dragon'] = dragon.get('health')
                    row['dragon_phase'] = dragon.get('phase')
                    row['wall'] = round(time.monotonic() - started)
                    trail.append(row)
                    note = (end.get('phase'), end.get('note'))
                    if note != last and args.verbose:
                        print(f'  [{seed} {level}] {row}', flush=True)
                    last = note
                    if end.get('phase') in ('done', 'failed'):
                        break
                out['end'] = end
                out['wall'] = round(time.monotonic() - started)
                if end.get('phase') not in ('done', 'failed') and 'failure' not in out:
                    out['failure'] = f'wall timeout in {end.get("phase")}: {end.get("note")}'
            except Exception as error:  # noqa: BLE001 - a run's crash is a result
                out['failure'] = f'harness: {error!r}'[:300]
            finally:
                stop(process)
            if args.keep:
                keep = args.keep / f'{seed}-{level}{"-beds" if args.beds else ""}'
                keep.mkdir(parents=True, exist_ok=True)
                log.seek(0)
                (keep / 'client.log').write_text(log.read())
                (keep / 'trail.json').write_text(json.dumps(trail, indent=1))
    end = out.get('end') or {}
    out['ok'] = end.get('phase') == 'done' or any(s[0] == 'dragon killed' for s in end.get('splits', []))
    if not out.get('failure') and end.get('phase') == 'failed':
        out['failure'] = end.get('failure')
    return out


def split(end, name):
    return next((t for n, t in end.get('splits', []) if n == name), None)


def table(results):
    print('\n| level | runs | dragon killed | run complete | time to kill (median, min-max) | deaths (causes) | crystals destroyed (mean) |')
    print('| --- | --- | --- | --- | --- | --- | --- |')
    for level in sorted({r['level'] for r in results}):
        rs = [r for r in results if r['level'] == level]
        killed = [r for r in rs if r['ok']]
        done = [r for r in rs if (r.get('end') or {}).get('phase') == 'done']
        times = []
        for r in killed:
            end = r['end']
            entered, kill = split(end, 'entered the End'), split(end, 'dragon killed')
            if kill is not None:
                times.append((kill - (entered or 0)) / 20)
        times.sort()
        deaths = [d for r in rs for d in (r.get('end') or {}).get('deaths', [])]
        causes = ', '.join(f'{c} x{deaths.count(c)}' for c in sorted(set(deaths))) or '-'
        crystals = [((r.get('end') or {}).get('destroyed') or 0) for r in rs]
        timing = (f'{times[len(times) // 2]:.0f} s ({times[0]:.0f}-{times[-1]:.0f})' if times else '-')
        print(f'| {level} | {len(rs)} | {len(killed)} | {len(done)} | {timing} | {len(deaths)} ({causes}) | '
              f'{sum(crystals) / max(1, len(crystals)):.1f} |')
    print()
    for r in results:
        end = r.get('end') or {}
        print(f'seed {r["seed"]:>4} {r["level"]:<5} {"OK  " if r["ok"] else "FAIL"} '
              f'phase={end.get("phase")} crystals={end.get("destroyed")} arrows={end.get("arrows")} '
              f'hits={end.get("hits")} beds={end.get("beds_used")} ({end.get("bed_damage")} dmg) '
              f'deaths={end.get("deaths")} splits={end.get("splits")} wall={r.get("wall")}s '
              f'{("failure: " + str(r.get("failure"))) if r.get("failure") else ""}')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--seeds', type=int, nargs='+', default=[1, 2, 3, 4, 5])
    parser.add_argument('--levels', nargs='+', default=['all', 'fair'])
    parser.add_argument('--beds', action='store_true', help='the bed one-cycle at perches')
    parser.add_argument('--pumpkin', action='store_true',
                        help='a carved pumpkin to wear (endermen ignore its gaze)')
    parser.add_argument('--armour', choices=('iron', 'none'), default='iron')
    parser.add_argument('--arrows', type=int, default=64)
    parser.add_argument('--jobs', type=int, default=2, help='clients at a time')
    parser.add_argument('--timeout', type=int, default=1800, help='wall seconds a run may take')
    parser.add_argument('--keep', type=Path, help='keep each run\'s log and status trail here')
    parser.add_argument('--results', type=Path, help='append each run as a JSON line here')
    parser.add_argument('--verbose', action='store_true')
    args = parser.parse_args()
    args.client, args.resources = args.client.resolve(), args.resources.resolve()
    runs = [(seed, level) for seed in args.seeds for level in args.levels]
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = {pool.submit(one_run, args, seed, level): (seed, level) for seed, level in runs}
        for future in concurrent.futures.as_completed(futures):
            r = future.result()
            results.append(r)
            end = r.get('end') or {}
            print(f'run seed={r["seed"]} level={r["level"]}: {"OK" if r["ok"] else "FAIL"} '
                  f'phase={end.get("phase")} splits={end.get("splits")} deaths={end.get("deaths")} '
                  f'failure={r.get("failure")}', flush=True)
            if args.results:
                with args.results.open('a') as f:
                    f.write(json.dumps(r) + '\n')
    results.sort(key=lambda r: (r['level'], r['seed']))
    table(results)
    if not all(r['ok'] for r in results):
        sys.exit(1)


if __name__ == '__main__':
    main()
