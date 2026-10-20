#!/usr/bin/env python3
"""Companion work skills end to end (docs/companions.md, "Skills"): on an
arena built with commands in a fixed-seed world, a companion chops 8 oak logs
from a small forest, makes a wooden pickaxe from nothing (planks, sticks, a
crafting table it places), mines 3 buried iron ore with a stone pickaxe
(after saying a wooden one won't do), smelts the raw iron in a furnace,
throws its owner 4 planks, goes to bed when its owner does, and fights a
skeleton with a bow (and a zombie up close with its sword). It also checks
that a work skill stops on command and comes back after a save and reload,
and reports what the work skills cost a tick.

    python3 tools/test_companion_skills.py --client target/fast/horizon-client [--offscreen] [--runs 3]

Linux needs a display (xvfb-run -a); macOS can pass --offscreen. Worlds and
settings live in a temporary directory; --shots keeps screenshots.
"""
import argparse
import json
import math
from pathlib import Path
import tempfile
import time

from test_companion import ROOT, SEED, command, companion, look_at, player, start, stop, wait_for

WORLD = 'CompanionSkills'
# The arena: a 33x33 grass platform on 8 blocks of stone, high above the
# terrain, so every run digs, walks and builds on the same blocks.
X, Y, Z = 300, 150, 300
TREES = ((X - 8, Z - 6), (X - 9, Z + 3), (X - 3, Z - 10))
ORES = ((X + 8, Y - 3, Z + 6), (X + 7, Y - 2, Z + 8), (X + 6, Y - 4, Z + 9))
# A fourth ore with lava behind it: it leaves that one alone.
LAVA_ORE, LAVA = (X + 9, Y - 4, Z + 7), (X + 10, Y - 4, Z + 7)


def say(endpoint, line):
    out = command(endpoint, f'command {line}')
    return out.get('chat', [])


def companion_do(endpoint, words):
    return command(endpoint, f'companion do {words}')


def counts(c):
    return c.get('inventory', {})


def count(c, item):
    return counts(c).get(f'minecraft:{item}', 0)


def owner_count(endpoint, item):
    return command(endpoint, 'inventory')['counts'].get(f'minecraft:{item}', 0)


class Reports:
    """Every report since the last call, kept in order."""

    def __init__(self, endpoint):
        self.endpoint = endpoint
        self.all = []

    def new(self):
        got = [r['text'] for r in command(self.endpoint, 'companion reports')['reports']]
        self.all.extend(got)
        return got

    def wait(self, predicate, timeout, what):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for line in self.new():
                if predicate(line):
                    return line
            time.sleep(0.25)
        raise AssertionError(f'timed out waiting for {what}: {self.all[-30:]} {companion(self.endpoint)}')


def build_arena(endpoint):
    """The platform, three oak trees, iron ore in the stone and room above."""
    say(endpoint, f'fill {X - 16} {Y + 1} {Z - 16} {X + 16} {Y + 20} {Z + 16} air')
    say(endpoint, f'fill {X - 16} {Y - 8} {Z - 16} {X + 16} {Y - 1} {Z + 16} stone')
    say(endpoint, f'fill {X - 16} {Y} {Z - 16} {X + 16} {Y} {Z + 16} grass_block')
    for tx, tz in TREES:
        say(endpoint, f'fill {tx - 2} {Y + 4} {tz - 2} {tx + 2} {Y + 6} {tz + 2} oak_leaves[persistent=true]')
        say(endpoint, f'fill {tx} {Y + 1} {tz} {tx} {Y + 5} {tz} oak_log')
    for x, y, z in (*ORES, LAVA_ORE):
        say(endpoint, f'setblock {x} {y} {z} iron_ore')
    say(endpoint, f'setblock {LAVA[0]} {LAVA[1]} {LAVA[2]} lava')


def owner_to(endpoint, x, z, creative=False):
    say(endpoint, 'gamemode creative')
    say(endpoint, f'tp @s {x + 0.5} {Y + 1} {z + 0.5}')
    wait_for(endpoint, 'status', lambda v: v['player'] and v['pending_chunks'] == 0, timeout=60,
             what='owner in place')
    if not creative:
        say(endpoint, 'gamemode survival')


def reset(endpoint, name, *items):
    """Its inventory emptied, then `items` given, at the arena's centre."""
    say(endpoint, f'clear {name}')
    for item in items:
        say(endpoint, f'give {name} {item}')
    say(endpoint, f'tp {name} {X + 0.5} {Y + 1} {Z + 2.5}')
    time.sleep(0.3)


def shot(endpoint, shots, name, close=False):
    """A screenshot of the companion without the HUD: from where its owner
    stands, or (`close`) from a spectator's spot 3 blocks off, for a
    companion out of sight in its mine."""
    if not shots:
        return
    back = None
    if close:
        p = player(endpoint)
        back = (p['position'], command(endpoint, 'state')['gamemode'])
        x, y, z = companion(endpoint)['position']
        say(endpoint, 'gamemode spectator')
        say(endpoint, f'tp @s {x - 2.2} {y + 0.6} {z - 2.2}')
        time.sleep(0.5)
    look_at(endpoint, companion(endpoint)['position'])
    command(endpoint, 'key f1 tap')
    time.sleep(0.3)
    command(endpoint, f'screenshot {shots / name}')
    command(endpoint, 'key f1 tap')
    if back:
        (x, y, z), mode = back
        say(endpoint, f'tp @s {x} {y} {z}')
        say(endpoint, f'gamemode {mode}')


def cost(endpoint):
    return command(endpoint, 'companion cost')['cost']


def scenario(endpoint, shots, home):
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title screen')
    command(endpoint, f'open {WORLD} {SEED}')
    wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
             timeout=180, what='world loaded')
    command(endpoint, 'time 1000')
    for rule in ('advance_time', 'spawn_mobs', 'spawn_patrols', 'spawn_phantoms', 'spawn_wandering_traders'):
        say(endpoint, f'gamerule {rule} false')
    say(endpoint, 'difficulty easy')
    owner_to(endpoint, X, Z, creative=True)
    build_arena(endpoint)
    owner_to(endpoint, X, Z)
    name = command(endpoint, 'companion spawn Ari')['name']
    # The arena's iron ore is buried: All-knowing finds it through the
    # stone (Fair, the default, only knows what it saw).
    companion_do(endpoint, f'{name} perception all')
    reports = Reports(endpoint)
    numbers = {}

    # Collect: 8 oak logs from the trees, by hand.
    reset(endpoint, name)
    cost(endpoint)
    companion_do(endpoint, 'collect 8 oak log')
    started = time.monotonic()
    reports.wait(lambda r: r == 'finished collect' or 'collect failed' in r, 180, 'logs')
    assert 'finished collect' in reports.all, reports.all[-20:]
    c = companion(endpoint)
    assert count(c, 'oak_log') >= 8, c
    numbers['collect'] = (time.monotonic() - started, cost(endpoint))
    print(f'collected {count(c, "oak_log")} oak logs in {numbers["collect"][0]:.0f} s')
    shot(endpoint, shots, 'companion-chop.png')

    # Make a wooden pickaxe from nothing: logs, planks, sticks, a table it places.
    reset(endpoint, name)
    companion_do(endpoint, 'make a wooden pickaxe')
    started = time.monotonic()
    reports.wait(lambda r: r == 'finished craft' and 'started craft' in reports.all or 'craft failed' in r,
                 240, 'pickaxe')
    c = companion(endpoint)
    assert count(c, 'wooden_pickaxe') == 1, (c, reports.all[-30:])
    tables = command(endpoint, f'find-block crafting_table 24')
    assert tables['count'] >= 1, tables
    numbers['make'] = (time.monotonic() - started, cost(endpoint))
    print(f'made a wooden pickaxe from nothing in {numbers["make"][0]:.0f} s '
          f'(gathered logs, placed a table at {tables["nearest"][0]["pos"]})')
    shot(endpoint, shots, 'companion-crafted.png')

    # Iron ore needs a stone pickaxe: it says so, then mines 3 with one.
    companion_do(endpoint, 'get 1 iron ore')
    failed = reports.wait(lambda r: 'collect failed' in r or r == 'finished collect', 30, 'tool check')
    assert failed == 'collect failed: needs a stone pickaxe', failed
    reset(endpoint, name, 'stone_pickaxe')
    companion_do(endpoint, 'get 3 iron ore')
    started = time.monotonic()
    if shots:
        reports.wait(lambda r: r.startswith('collect 1/3') or 'collect failed' in r, 240, 'first ore')
        shot(endpoint, shots, 'companion-mining.png', close=True)
    reports.wait(lambda r: r == 'finished collect' or 'collect failed' in r, 240, 'iron ore')
    c = companion(endpoint)
    assert count(c, 'raw_iron') >= 3, (c, reports.all[-30:])
    assert not c['dead'] and c['health'] > 10, c
    numbers['ore'] = (time.monotonic() - started, cost(endpoint))
    print(f'mined 3 buried iron ore in {numbers["ore"][0]:.0f} s, health {c["health"]:.0f}')
    # The one beside lava is left alone, and it says why.
    companion_do(endpoint, 'get 1 iron ore')
    failed = reports.wait(lambda r: 'collect failed' in r or r == 'finished collect', 60, 'lava ore')
    assert failed == 'collect failed: no iron ore within 48 blocks I can dig safely (lava beside it)', failed
    lx, ly, lz = LAVA_ORE
    assert command(endpoint, f'block {lx} {ly} {lz}')['block'].startswith('minecraft:iron_ore'), 'lava ore dug'
    print('left the iron ore beside lava alone')

    # Smelt the raw iron in a furnace nearby, with coal.
    say(endpoint, f'setblock {X + 3} {Y + 1} {Z - 3} furnace[facing=south]')
    say(endpoint, f'give {name} coal 2')
    companion_do(endpoint, 'smelt 3 raw iron')
    started = time.monotonic()
    reports.wait(lambda r: r == 'finished smelt' or 'smelt failed' in r, 120, 'smelting')
    c = companion(endpoint)
    assert count(c, 'iron_ingot') >= 3, (c, reports.all[-30:])
    numbers['smelt'] = (time.monotonic() - started, cost(endpoint))
    print(f'smelted 3 iron ingots in {numbers["smelt"][0]:.0f} s')

    # Give: 4 planks thrown to its owner.
    reset(endpoint, name, 'oak_planks 6')
    before = owner_count(endpoint, 'oak_planks')
    companion_do(endpoint, 'give me 4 planks')
    reports.wait(lambda r: r == 'finished give' or 'give failed' in r, 60, 'give')
    wait_for(endpoint, 'inventory', lambda v: v['counts'].get('minecraft:oak_planks', 0) >= before + 4,
             timeout=20, what='planks picked up')
    time.sleep(2.5)
    c = companion(endpoint)
    assert owner_count(endpoint, 'oak_planks') == before + 4, command(endpoint, 'inventory')['counts']
    assert count(c, 'oak_planks') == 2, c
    print('threw its owner exactly 4 planks')

    # Work stops on command, and a saved world takes it up again.
    reset(endpoint, name)
    companion_do(endpoint, 'collect 3 oak log')
    reports.wait(lambda r: r == 'started collect', 10, 'collect start')
    companion_do(endpoint, 'stop')
    reports.wait(lambda r: r == 'collect failed: stopped', 10, 'stop')
    assert companion(endpoint)['task'] is None, companion(endpoint)
    companion_do(endpoint, 'collect 3 oak log')
    reports.wait(lambda r: r == 'started collect', 10, 'collect again')
    command(endpoint, 'key esc tap')
    wait_for(endpoint, 'screen', lambda v: any(b['label'] == 'Save and Quit to Title'
                                               for b in v.get('buttons', [])), what='pause menu')
    command(endpoint, 'click-button Save and Quit to Title')
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title')
    saved = json.loads(next((home / 'saves' / WORLD / 'companions').glob('*.json')).read_text())
    assert saved['task'] and saved['task'].startswith('collect '), saved
    command(endpoint, f'open {WORLD}')
    back = wait_for(endpoint, 'companion list', lambda v: v['companions'] and v['companions'][0].get('task'),
                    timeout=120, what='work taken up again')['companions'][0]
    assert back['task']['skill'] == 'collect', back
    reports.wait(lambda r: r == 'finished collect' or 'collect failed' in r, 180, 'resumed collect')
    assert 'finished collect' in reports.all[-5:], reports.all[-10:]
    print(f'stopped on command; saved as "{saved["task"]}" and finished after the reload')

    # Sleep: night, two beds; the owner goes to bed and so does it.
    say(endpoint, 'time set 13000')
    say(endpoint, f'setblock {X + 2} {Y + 1} {Z + 6} red_bed[part=foot,facing=south]')
    say(endpoint, f'setblock {X + 2} {Y + 1} {Z + 7} red_bed[part=head,facing=south]')
    say(endpoint, f'setblock {X + 6} {Y + 1} {Z + 6} blue_bed[part=foot,facing=south]')
    say(endpoint, f'setblock {X + 6} {Y + 1} {Z + 7} blue_bed[part=head,facing=south]')
    say(endpoint, f'tp @s {X + 2.5} {Y + 1} {Z + 4.5} 0 40')
    time.sleep(0.5)
    command(endpoint, 'mouse right click')
    wait_for(endpoint, 'state', lambda v: v.get('sleeping'), timeout=10, what='owner in bed')
    wait_for(endpoint, 'companion list', lambda v: v['companions'][0]['sleeping'], timeout=30,
             what='companion in bed')
    print('went to bed beside its owner')
    shot(endpoint, shots, 'companion-asleep.png')
    wait_for(endpoint, 'companion list', lambda v: not v['companions'][0]['sleeping'], timeout=30,
             what='morning')
    reports.wait(lambda r: r == 'finished sleep', 10, 'sleep finished')
    print('woke up in the morning with its owner')

    # Bow: a skeleton 14 blocks away, then a zombie up close.
    say(endpoint, 'time set 18000')
    owner_to(endpoint, X - 6, Z - 12, creative=True)
    reset(endpoint, name, 'bow', 'arrow 32', 'iron_sword', 'iron_chestplate', 'iron_helmet')
    say(endpoint, f'tp {name} {X + 0.5} {Y + 1} {Z + 0.5} 0 0')
    companion_do(endpoint, 'stay')
    time.sleep(1.5)
    skeleton = command(endpoint, f'summon skeleton {X + 0.5} {Y + 1} {Z + 14.5}')['id']
    companion_do(endpoint, 'attack nearest')
    if shots:
        time.sleep(0.8)
        shot(endpoint, shots, 'companion-bow.png')
    wait_for(endpoint, 'mobs 64', lambda v: not any(m['id'] == skeleton and m['health'] > 0 for m in v['near']),
             timeout=60, what='skeleton dead')
    c = companion(endpoint)
    arrows = count(c, 'arrow')
    assert arrows < 32, c
    print(f'shot the skeleton dead with {32 - arrows} arrows (health {c["health"]:.0f})')
    zombie = command(endpoint, f'summon zombie {X + 0.5} {Y + 1} {Z + 3.5}')['id']
    companion_do(endpoint, 'attack nearest')
    wait_for(endpoint, 'mobs 64', lambda v: not any(m['id'] == zombie and m['health'] > 0 for m in v['near']),
             timeout=60, what='zombie dead')
    c = companion(endpoint)
    assert c['held'] == 'minecraft:iron_sword', c
    assert count(c, 'arrow') >= arrows - 1, c
    print('fought the zombie up close with its sword')

    command(endpoint, 'companion dismiss')
    command(endpoint, 'quit')
    return numbers, reports.all


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--shots', type=Path, help='keep screenshots in this folder')
    parser.add_argument('--runs', type=int, default=1)
    args = parser.parse_args()
    client, resources = args.client.resolve(), args.resources.resolve()
    shots = args.shots.resolve() if args.shots else None
    if shots:
        shots.mkdir(parents=True, exist_ok=True)
    for run in range(args.runs):
        with tempfile.TemporaryDirectory(prefix='horizon-companion-skills-') as directory:
            home = Path(directory) / 'game'
            home.mkdir()
            endpoint = home / 'control.sock'
            with (home / 'client.log').open('w+') as log:
                process = start(client, resources, home, endpoint, log, args.offscreen)
                try:
                    numbers, events = scenario(endpoint, shots if run == 0 else None, home)
                    assert process.wait(timeout=30) == 0, 'client exited unsuccessfully'
                except Exception:
                    log.seek(0)
                    print('\n'.join(log.read().splitlines()[-40:]))
                    raise
                finally:
                    stop(process)
        print(f'run {run + 1}: ' + ', '.join(
            f'{k} {t:.0f} s (work {c["work_mean_ms"]:.3f} ms mean, {c["work_max_ms"]:.2f} worst; '
            f'hub {c["mean_ms"]:.3f} mean, {c["max_ms"]:.2f} worst; worst path search {c.get("plan_max_ms", 0):.2f})'
            for k, (t, c) in numbers.items()))
    print('Companion skills passed: collect, make from nothing, tool check, mining, smelt, give, '
          'stop and resume, sleep, bow and melee')


if __name__ == '__main__':
    main()
