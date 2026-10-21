#!/usr/bin/env python3
"""Nether combat lab: one fighter, fixed gear, a closed arena in the Nether, every hit recorded.

Each run opens a fresh world (difficulty Normal), takes the fighter into the Nether through a portal and
walls a flat nether brick arena round it (closed, so no lava or other mobs come in; the other Nether mobs
that still spawn inside are removed every slice). The fighter has iron armour (15 points), a shield, an
iron sword, an iron pickaxe, 32 cooked beef and blocks. Then the scenario runs under /tick sprint in
slices, and the game's `hurtlog` (every hurt the fighter took: damage type, amount, what the shield
blocked, what it took, the armour it met, its fire ticks, the knockback; every melee blow: damage,
attack strength, crit, kill) is read after each slice.

Fighters (--subject):
- companion: the companion (a LAN guest driven by the server), the owner watching it. In `spawner` and
             `summon` it runs the Speedrunner's blaze leg (`nether blazes`); in `passive` it stays and
             holds fire.
- host:      the host player itself (no companion), standing where it was put; with --shield it holds
             the shield up toward the mob. Only `passive`: the engine baseline the companion is compared
             with.

Scenarios (--scenario):
- spawner:  a blaze spawner 6 blocks off: kill --kills blazes (default 6) without dying.
- summon:   up to three blazes at a time are summoned 8-10 blocks off until --kills are dead.
- mixed:    two blazes, two magma cubes and a ghast in a taller arena; kill --kills blazes.
- passive:  one blaze 8 blocks off for --ticks ticks (default 600): what one blaze does to a fighter that
            doesn't fight back.
- bridge:   the open: a 3-wide netherrack bridge 48 blocks long over a lava lake (both sides, in a closed
            nether brick hall), fire burning on a few of its cells, a spawner (of pigs) at its far end.
            The companion runs the blaze leg, so it walks the bridge to the spawner, while two blazes at a
            time are summoned over the lava beside it, ahead, and a ghast floats over the lake. Done when it
            stands within 6 blocks of the spawner.
- walk:     the same bridge, no mobs: the walker alone (fire and lava on its path).
- fireres:  vanilla's fire resistance on a LAN guest: the companion (Fire Resistance on, holding still,
            no shield) stands by a blaze for --ticks ticks, then in fire, then in lava. Every hurt of a
            #minecraft:is_fire type it takes must be refused (no health lost, nothing taken).

    python3 tools/test_companion_lab.py --client target/fast/horizon-client --offscreen \\
        --subject companion --scenario spawner --runs 10 [--parallel 3]

Each run's record goes to .local/companion-lab/<subject>-<scenario>-<n>.json; the summary (survival,
kills, time, damage per hit by source, blocked share, armour seen, fire, attacks) is printed and written to
.local/companion-lab/report-<subject>-<scenario>.md.
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
from test_companion_nether import NAME, block, into_nether, portal_by, runner, settle, sprint, stop

SEED = 42
ARMOUR = ['iron_helmet', 'iron_chestplate', 'iron_leggings', 'iron_boots']
KIT = ARMOUR + ['iron_sword', 'iron_pickaxe', 'shield', 'cooked_beef 32', 'cobblestone 64', 'netherrack 64']
# Nether mobs that are not the lab's: removed every slice.
STRAYS = ['zombified_piglin', 'piglin', 'piglin_brute', 'hoglin', 'strider', 'enderman', 'skeleton',
          'wither_skeleton']
BUDGET = {'spawner': 9600, 'summon': 9600, 'mixed': 9600, 'bridge': 6000, 'walk': 6000, 'barter': 9600}
# The bridge: its length, the hall's half width and height over the bridge.
BRIDGE = 48
OPEN = ('bridge', 'walk')


def start(client, resources, home, endpoint, log, offscreen, timeout):
    extra = ['--offscreen'] if offscreen else []
    return subprocess.Popen([
        str(client), '--resources', str(resources), '--render-distance', '6',
        '--control-socket', str(endpoint), '--timeout-seconds', str(timeout), *extra,
    ], cwd=home, env=dict(os.environ, HORIZON_HOME=str(home), HORIZON_NO_AUDIO='1',
                         HORIZON_MENU_STYLE='classic', HORIZON_PLAYER_NAME='Owner'),
        stdout=log, stderr=subprocess.STDOUT)


def host_into_nether(endpoint):
    """The host alone walks into a portal built beside it."""
    s = command(endpoint, 'status')['player']
    x, y, z = (math.floor(v) for v in s['position'])
    pz = z + 3
    command(endpoint, f'command fill {x - 3} {y} {z - 1} {x + 4} {y + 5} {z + 5} air')
    command(endpoint, f'command fill {x - 3} {y - 1} {z - 1} {x + 4} {y - 1} {z + 5} stone')
    command(endpoint, f'command fill {x - 1} {y - 1} {pz} {x + 2} {y + 3} {pz} obsidian')
    command(endpoint, f'command fill {x} {y} {pz} {x + 1} {y + 2} {pz} nether_portal[axis=x]')
    command(endpoint, f'command tp @s {x + 1.0} {y} {pz + 0.5}')
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        s = command(endpoint, 'status')
        if s['dimension'] == 'minecraft:the_nether' and s['pending_chunks'] == 0:
            return
        time.sleep(0.3)
    raise AssertionError('the host did not get through the portal')


def arena(endpoint, at, height):
    """A closed nether brick box round `at` (its floor under `at`), empty inside."""
    x, y, z = at
    r = 16
    command(endpoint, f'command fill {x - r - 1} {y - 2} {z - r - 1} {x + r + 1} {y + height} {z + r + 1} '
                      f'nether_bricks hollow')
    command(endpoint, f'command fill {x - r} {y - 1} {z - r} {x + r} {y - 1} {z + r} nether_bricks')
    command(endpoint, f'command kill @e[type=!minecraft:player,x={x},y={y},z={z},distance=..40]')


def bridge(endpoint, at):
    """A hall of nether bricks along +x from `at`: a lava lake two deep, a 3-wide netherrack bridge across it
    at the floor's level, a platform at each end, fire on a few bridge cells, a pig spawner on the far one."""
    x, y, z = at
    far = x + BRIDGE
    command(endpoint, f'command fill {x - 5} {y - 4} {z - 9} {far + 7} {y + 14} {z + 9} nether_bricks hollow')
    command(endpoint, f'command fill {x - 4} {y - 3} {z - 8} {far + 6} {y - 3} {z + 8} nether_bricks')
    command(endpoint, f'command fill {x - 4} {y - 2} {z - 8} {far + 6} {y - 1} {z + 8} lava')
    command(endpoint, f'command fill {x - 4} {y - 1} {z - 1} {far + 6} {y - 1} {z + 1} netherrack')
    command(endpoint, f'command fill {x - 4} {y - 1} {z - 3} {x + 2} {y - 1} {z + 3} netherrack')
    command(endpoint, f'command fill {far} {y - 1} {z - 3} {far + 6} {y - 1} {z + 3} netherrack')
    for fx, fz in [(x + 12, z), (x + 20, z - 1), (x + 28, z), (x + 28, z + 1), (x + 36, z + 1)]:
        command(endpoint, f'command setblock {fx} {y} {fz} fire')
    command(endpoint, f'command setblock {far + 3} {y} {z} minecraft:spawner'
                      '{SpawnData:{entity:{id:"minecraft:pig"}}}')
    command(endpoint, f'command kill @e[type=!minecraft:player,x={x + BRIDGE // 2},y={y},z={z},distance=..60]')


def strays(endpoint, at, scenario):
    x, y, z = at
    kinds = STRAYS + ([] if scenario in ('mixed', 'bridge') else ['ghast', 'magma_cube'])
    if scenario == 'barter':
        kinds = [k for k in kinds if k != 'piglin']
    for kind in kinds:
        try:
            command(endpoint, f'command kill @e[type=minecraft:{kind},x={x},y={y},z={z},distance=..48]')
        except AssertionError:
            pass  # none there ("No entity was found")


def blazes(endpoint, at, scenario=''):
    x, y, z = at
    mobs = command(endpoint, 'mobs 64')['near']
    far = BRIDGE + 8 if scenario in OPEN else 20
    return [m for m in mobs if m['kind'].endswith('blaze') and m['health'] > 0
            and -20 < m['pos'][0] - x < far and abs(m['pos'][2] - z) < 20]


def summon(endpoint, kind, at, n, distance):
    x, y, z = at
    for i in range(n):
        a = (time.monotonic() * 1.7 + i * 2.1) % (2 * math.pi)
        d = distance[0] + (distance[1] - distance[0]) * ((i * 0.37) % 1)
        command(endpoint, f'command summon minecraft:{kind} {x + 0.5 + math.cos(a) * d:.1f} {y + 1} '
                          f'{z + 0.5 + math.sin(a) * d:.1f}')


def fighter(endpoint, subject):
    """(health, food, dead, position, beef left) of the fighter."""
    if subject == 'host':
        s = command(endpoint, 'status')['player']
        return s.get('health'), s.get('food'), s.get('health', 1) <= 0, s.get('position'), None
    c = runner(endpoint)
    beef = (c.get('inventory') or {}).get('minecraft:cooked_beef', 0)
    return c['health'], c['food'], c['dead'], c['position'], beef


def setup(endpoint, args, n):
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title screen')
    command(endpoint, f'open Lab{n} {args.seed}')
    settle(endpoint, 'world loaded', timeout=240)
    command(endpoint, 'command difficulty peaceful')
    command(endpoint, 'time 1000')
    spawn = command(endpoint, 'status')['player']['world_spawn']
    command(endpoint, f'command tp @s {spawn[0] + 0.5} {spawn[1] + 1} {spawn[2] + 0.5}')
    settle(endpoint, 'owner at the spawn')
    if args.subject == 'host':
        host_into_nether(endpoint)
        settle(endpoint, 'host in the Nether')
        p = command(endpoint, 'status')['player']['position']
        at = tuple(math.floor(v) for v in p)
        arena(endpoint, at, args.height)
        command(endpoint, f'command tp @s {at[0] + 0.5} {at[1]} {at[2] + 0.5} -90 0')
        for i, piece in enumerate(ARMOUR):
            slot = ['head', 'chest', 'legs', 'feet'][i]
            command(endpoint, f'command item replace entity @s armor.{slot} with minecraft:{piece}')
        command(endpoint, 'command item replace entity @s weapon.mainhand with minecraft:iron_sword')
        command(endpoint, 'command item replace entity @s weapon.offhand with minecraft:shield')
        command(endpoint, 'command give @s minecraft:cooked_beef 32')
        return at
    command(endpoint, f'companion spawn {NAME}')
    wait_for(endpoint, 'companion list', lambda v: runner(endpoint) and not runner(endpoint)['dead'],
             what='the runner')
    command(endpoint, f'companion do {NAME} stay')
    time.sleep(0.5)
    for item in KIT:
        command(endpoint, f'command give {NAME} {item}')
    command(endpoint, f'companion watch {NAME}')
    into_nether(endpoint, portal_by(endpoint))
    settle(endpoint, 'in the Nether')
    c = runner(endpoint)
    at = tuple(math.floor(v) for v in c['position'])
    if args.scenario in OPEN:
        bridge(endpoint, at)
    else:
        arena(endpoint, at, args.height)
    command(endpoint, f'command tp {NAME} {at[0] + 0.5} {at[1]} {at[2] + 0.5} -90 0')
    # The kit on: its armour goes on by itself (best armour), a tick or two.
    sprint(endpoint, 20)
    return at


def one_run(endpoint, args, n):
    at = setup(endpoint, args, n)
    x, y, z = at
    scenario = args.scenario
    if args.potion and args.subject == 'companion':
        command(endpoint, f'command give {NAME} minecraft:potion[minecraft:potion_contents='
                          '{potion:"minecraft:fire_resistance"}]')
    command(endpoint, 'hurtlog')  # what the setup did
    command(endpoint, 'command difficulty normal')
    result = {'subject': args.subject, 'scenario': scenario, 'seed': args.seed, 'run': n,
              'shield': args.shield if args.subject == 'host' else True}
    if scenario == 'spawner':
        command(endpoint, f'command setblock {x + 6} {y} {z} minecraft:spawner'
                          '{SpawnData:{entity:{id:"minecraft:blaze"}}}')
    elif scenario == 'mixed':
        summon(endpoint, 'blaze', at, 2, (8, 10))
        summon(endpoint, 'magma_cube', at, 2, (6, 8))
        command(endpoint, f'command summon minecraft:ghast {x + 12.5} {y + 10} {z + 0.5}')
    elif scenario == 'passive':
        command(endpoint, f'command summon minecraft:blaze {x + 8.5} {y} {z + 0.5}')
    elif scenario == 'fireres':
        command(endpoint, f'command effect give {NAME} minecraft:fire_resistance 600 0')
        command(endpoint, f'command item replace entity {NAME} weapon.offhand with minecraft:air')
        command(endpoint, f'command summon minecraft:blaze {x + 8.5} {y} {z + 0.5}')
    elif scenario == 'bridge':
        command(endpoint, f'command summon minecraft:ghast {x + BRIDGE // 2 + 0.5} {y + 8} {z + 6.5}')
    elif scenario == 'barter':
        # Gold boots on (piglins stay calm), gold blocks to craft into ingots, piglins a few blocks off.
        command(endpoint, f'command clear {NAME} minecraft:iron_boots')
        command(endpoint, f'command give {NAME} minecraft:golden_boots')
        command(endpoint, f'command give {NAME} minecraft:gold_block {args.gold}')
        sprint(endpoint, 40)  # its best boots on: the gold ones
        for i in range(args.piglins):
            command(endpoint, f'command summon minecraft:piglin {x + 6.5} {y} {z + 0.5 + 3 * (i - 1)} '
                              '{IsImmuneToZombification:1b}')
    goal = (x + BRIDGE + 3, y, z)
    result['goal'] = list(goal)
    if args.subject == 'companion':
        if scenario in ('passive', 'fireres'):
            command(endpoint, f'companion do {NAME} stay')
            command(endpoint, f'companion do {NAME} hold')
        elif scenario == 'barter':
            command(endpoint, f'companion do {NAME} nether barter until barter all_knowing')
        else:
            command(endpoint, f'companion do {NAME} nether blazes until blazes all_knowing rods 64')
    elif scenario == 'passive':
        command(endpoint, f'look -90 0')
        if args.shield:
            command(endpoint, 'mouse right down')
    hurts, attacks, samples = [], [], []
    started = time.monotonic()
    done = 0
    summoned = 0
    budget = args.ticks if scenario in ('passive', 'fireres') else BUDGET[scenario]
    lit = 0
    first_beef = None
    death = None
    while done < budget and time.monotonic() - started < args.wall:
        if scenario == 'bridge' and summoned < args.kills + 4:
            # Two at a time over the lava beside the bridge, ahead of it.
            c = runner(endpoint)
            cx = c['position'][0] if c else x
            alive = len(blazes(endpoint, at, scenario))
            for i in range(max(0, 2 - alive)):
                side = 1 if (summoned + i) % 2 else -1
                ahead = min(cx + 7 + 3 * ((summoned + i) % 3), x + BRIDGE)
                command(endpoint, f'command summon minecraft:blaze {ahead:.1f} {y + 2} {z + 0.5 + side * 6}')
                summoned += 1
        if scenario == 'summon':
            alive = len(blazes(endpoint, at, scenario))
            kills = sum(1 for a in attacks if a['killed'] and a['target'].endswith('blaze'))
            want = min(3 - alive, args.kills - kills - alive)
            if want > 0:
                summon(endpoint, 'blaze', at, want, (8, 10))
                summoned += want
        if scenario == 'passive' and args.subject == 'host':
            # Facing the blaze (it rises and drifts).
            b = blazes(endpoint, at, scenario)
            p = command(endpoint, 'status')['player']['position']
            if b:
                bx, by, bz = b[0]['pos']
                dx, dy, dz = bx - p[0], by + 0.9 - (p[1] + 1.62), bz - p[2]
                yaw = math.degrees(math.atan2(-dx, dz))
                pitch = -math.degrees(math.atan2(dy, math.hypot(dx, dz)))
                command(endpoint, f'look {yaw:.1f} {pitch:.1f}')
        if scenario == 'fireres' and done >= budget // 3 and lit == 0:
            # In fire, then (a third later) in lava.
            command(endpoint, f'command setblock {x} {y} {z} minecraft:fire')
            lit = 1
        if scenario == 'fireres' and done >= 2 * budget // 3 and lit == 1:
            command(endpoint, f'command setblock {x} {y} {z} minecraft:lava')
            lit = 2
        sprint(endpoint, args.slice)
        done += args.slice
        strays(endpoint, at, scenario)
        log = command(endpoint, 'hurtlog')['players']
        who = 'host' if args.subject == 'host' else NAME
        for p in log:
            if p['name'] == who:
                hurts += p['hurts']
                attacks += p['attacks']
        health, food, dead, pos, beef = fighter(endpoint, args.subject)
        if first_beef is None:
            first_beef = beef
        samples.append({'t': done, 'health': health, 'food': food, 'pos': pos, 'beef': beef,
                        'blazes': len(blazes(endpoint, at, scenario))})
        if args.subject == 'companion':
            nether = runner(endpoint).get('nether') or {}
            samples[-1]['note'] = nether.get('note')
            samples[-1]['step'] = nether.get('step')
        kills = sum(1 for a in attacks if a['killed'] and a['target'].endswith('blaze'))
        if dead or (hurts and hurts[-1]['health'] <= 0):
            death = hurts[-1]['source'] if hurts else 'unknown'
            break
        if scenario == 'fireres':
            pass
        elif scenario == 'barter':
            nether = runner(endpoint).get('nether') or {}
            if nether.get('outcome'):
                break
        elif scenario in OPEN:
            if pos and math.hypot(pos[0] - goal[0] - 0.5, pos[2] - goal[2] - 0.5) < 6:
                result['reached'] = done
                break
        elif scenario != 'passive' and kills >= args.kills:
            break
    if args.subject == 'host' and args.shield:
        command(endpoint, 'mouse right up')
    kills = sum(1 for a in attacks if a['killed'] and a['target'].endswith('blaze'))
    alive_samples = [s for s in samples if s['beef'] is not None and s['health'] and s['health'] > 0]
    if args.subject == 'companion':
        result['spawner'] = (runner(endpoint).get('nether') or {}).get('spawner')
        result['barter'] = (runner(endpoint).get('nether') or {}).get('barter')
        result['inventory'] = runner(endpoint).get('inventory')
    result.update({
        'survived': death is None,
        'death': death,
        'kills': kills,
        'summoned': summoned,
        'ticks': done,
        'wall_s': round(time.monotonic() - started, 1),
        'min_health': min((s['health'] for s in samples if s['health'] is not None), default=None),
        # (Before a death: the body drops it all.)
        'beef_eaten': (first_beef - min(s['beef'] for s in alive_samples)) if first_beef is not None
        and alive_samples else None,
        'hurts': hurts,
        'attacks': attacks,
        'samples': samples,
    })
    if args.subject == 'companion':
        result['reports'] = [r['text'] for r in command(endpoint, 'companion reports')['reports']][-60:]
        command(endpoint, 'companion watch off')
    command(endpoint, 'quit')
    return result


def run_one(args, n):
    results = ROOT / '.local/companion-lab'
    results.mkdir(parents=True, exist_ok=True)
    name = f'{args.subject}-{args.scenario}{"-noshield" if not args.shield else ""}-{n}'
    with tempfile.TemporaryDirectory(prefix='horizon-lab-') as directory:
        home = Path(directory) / 'game'
        home.mkdir()
        endpoint = home / 'control.sock'
        with (home / 'client.log').open('w+') as log:
            process = start(args.client, args.resources, home, endpoint, log, args.offscreen,
                            timeout=args.wall + 600)
            try:
                result = one_run(endpoint, args, n)
                process.wait(timeout=60)
            except Exception as e:  # a harness or game failure, kept with the log's end
                log.seek(0)
                result = {'subject': args.subject, 'scenario': args.scenario, 'run': n, 'error': repr(e),
                          'log': log.read().splitlines()[-40:], 'survived': False, 'kills': 0,
                          'hurts': [], 'attacks': []}
            finally:
                stop(process)
            log.seek(0)
            lines = [l for l in log.read().splitlines() if '[companion]' in l or 'panick' in l]
            (results / f'{name}.log').write_text('\n'.join(lines[-3000:]))
            # What it was doing at each hit a mob landed.
            notes = {}
            for l in lines:
                if 'nether hurt by' in l and 'while: ' in l:
                    k = l.split('nether hurt by ', 1)[1].split(' while: ')[0].replace('minecraft:', '') + ': ' + \
                        l.split('while: ', 1)[1]
                    notes[k] = notes.get(k, 0) + 1
            result['hurt_notes'] = notes
    (results / f'{name}.json').write_text(json.dumps(result, indent=1))
    print(json.dumps({k: result.get(k) for k in ('run', 'survived', 'death', 'kills', 'ticks', 'reached',
                                                  'min_health', 'beef_eaten', 'spawner', 'goal', 'barter', 'error')}),
          flush=True)
    return result


def mean(v):
    return round(statistics.mean(v), 2) if v else '-'


def summary(args, results):
    fire = ('minecraft:fireball', 'minecraft:on_fire', 'minecraft:in_fire', 'minecraft:lava')
    if args.scenario == 'fireres':
        for r in results:
            r['fire_taken'] = round(sum(h['taken'] for h in r['hurts'] if h['source'] in fire), 2)
            r['fire_hurts'] = sum(1 for h in r['hurts'] if h['source'] in fire)
            r['fire_refused'] = sum(1 for h in r['hurts'] if h['source'] in fire and not h['hurt'])
    ok = [r for r in results if r.get('survived') and (r.get('fire_taken', 0) == 0) and (
        r.get('reached') is not None if args.scenario in OPEN
        else args.scenario in ('passive', 'fireres', 'barter') or r['kills'] >= args.kills)]
    if args.scenario == 'fireres':
        lines_extra = [f'Run {r["run"]}: fire-type hurts {r["fire_hurts"]}, refused {r["fire_refused"]}, '
                       f'health taken {r["fire_taken"]}, min health {r.get("min_health")}' for r in results]
    else:
        lines_extra = []
    lines = [f'### {args.subject}, {args.scenario}{", no shield" if not args.shield else ""} '
             f'({len(results)} runs, seed {args.seed})', '',
             '| Survived and done | Deaths | Kills (mean) | Time (ticks, median of successes) | Min health (mean) '
             '| Beef eaten (mean) |', '| --- | --- | --- | --- | --- | --- |',
             f'| {len(ok)}/{len(results)} | {sum(1 for r in results if r.get("death"))} | '
             f'{mean([r["kills"] for r in results])} | '
             f'{statistics.median([r["ticks"] for r in ok]) if ok else "-"} | '
             f'{mean([r["min_health"] for r in results if r.get("min_health") is not None])} | '
             f'{mean([r["beef_eaten"] for r in results if r.get("beef_eaten") is not None])} |', ''] + lines_extra
    deaths = {}
    for r in results:
        if r.get('death'):
            deaths[r['death']] = deaths.get(r['death'], 0) + 1
        if r.get('error'):
            deaths['harness: ' + r['error'][:60]] = deaths.get('harness: ' + r['error'][:60], 0) + 1
    if deaths:
        lines += ['| Death / error | Runs |', '| --- | --- |']
        lines += [f'| {k} | {v} |' for k, v in sorted(deaths.items(), key=lambda kv: -kv[1])]
        lines.append('')
    by = {}
    for r in results:
        for h in r['hurts']:
            by.setdefault(h['source'], []).append(h)
    if by:
        lines += ['| Damage type | Hurts | Raw (mean) | Blocked share | Taken (mean) | Taken total/run | Armour seen '
                  '| Fire ticks before (mean) | Knockback (mean horiz.) |',
                  '| --- | --- | --- | --- | --- | --- | --- | --- | --- |']
        for source, hs in sorted(by.items(), key=lambda kv: -sum(h['taken'] for h in kv[1])):
            kb = [math.hypot(h['knockback'][0], h['knockback'][2]) for h in hs if h.get('knockback')]
            lines.append(
                f'| {source.replace("minecraft:", "")} | {len(hs)} | {mean([h["amount"] for h in hs])} | '
                f'{sum(1 for h in hs if h["blocked"] > 0)}/{len(hs)} | {mean([h["taken"] for h in hs])} | '
                f'{round(sum(h["taken"] for h in hs) / len(results), 1)} | '
                f'{sorted({h["armor"] for h in hs})} | {mean([max(h["fire_ticks"], 0) for h in hs])} | '
                f'{mean(kb)} |')
        lines.append('')
    shots = [h for r in results for h in r['hurts'] if h['source'] in ('minecraft:fireball', 'minecraft:fireball_ghast')
             or 'fireball' in h['source']]
    if shots:
        why = {}
        for h in shots:
            if h['blocked'] > 0:
                k = 'blocked'
            elif h.get('using') is None:
                k = 'shield down'
            elif h['using'] < 5:
                k = 'shield rising (<5 ticks)'
            elif h.get('angle') is not None and h['angle'] > 90:
                k = 'from the side or behind (>90 deg)'
            else:
                k = 'up, facing, not blocked'
            why[k] = why.get(k, 0) + 1
        lines += ['| Fireball hits | Runs |', '| --- | --- |']
        lines += [f'| {k} | {v} |' for k, v in sorted(why.items(), key=lambda kv: -kv[1])]
        angles = [round(h['angle']) for h in shots if h.get('angle') is not None and h['blocked'] <= 0]
        lines += ['', f'Angles of the unblocked: {sorted(angles)}', '']
    notes = {}
    for r in results:
        for k, v in (r.get('hurt_notes') or {}).items():
            notes[k] = notes.get(k, 0) + v
    if notes:
        lines += ['| Hit by a mob while | Hits |', '| --- | --- |']
        lines += [f'| {k} | {v} |' for k, v in sorted(notes.items(), key=lambda kv: -kv[1])[:20]]
        lines.append('')
    attacks = [a for r in results for a in r['attacks']]
    if attacks:
        lines += ['| Blows | Landed | Mean damage | Mean strength | Crits | Kills |', '| --- | --- | --- | --- | --- | --- |',
                  f'| {len(attacks)} | {sum(1 for a in attacks if a["landed"])} | '
                  f'{mean([a["amount"] for a in attacks])} | {mean([a["scale"] for a in attacks])} | '
                  f'{sum(1 for a in attacks if a["crit"])} | {sum(1 for a in attacks if a["killed"])} |', '']
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--subject', choices=['companion', 'host'], default='companion')
    parser.add_argument('--scenario', choices=['spawner', 'summon', 'mixed', 'passive', 'bridge', 'walk', 'fireres', 'barter'], default='spawner')
    parser.add_argument('--runs', type=int, default=10)
    parser.add_argument('--seed', type=int, default=SEED)
    parser.add_argument('--parallel', type=int, default=1)
    parser.add_argument('--kills', type=int, default=6)
    parser.add_argument('--potion', action='store_true', help='give the companion a fire resistance potion')
    parser.add_argument('--gold', type=int, default=6, help='barter: gold blocks given (9 ingots each)')
    parser.add_argument('--piglins', type=int, default=3, help='barter: piglins summoned')
    parser.add_argument('--ticks', type=int, default=600, help='passive: how long')
    parser.add_argument('--slice', type=int, default=100)
    parser.add_argument('--wall', type=int, default=600, help='wall seconds a run may take')
    parser.add_argument('--no-shield', dest='shield', action='store_false',
                        help='host: the shield stays down')
    parser.add_argument('--tag', default='', help='a suffix for the report file')
    args = parser.parse_args()
    args.height = 22 if args.scenario == 'mixed' else 12
    args.client, args.resources = args.client.resolve(), args.resources.resolve()
    with futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
        results = list(pool.map(lambda n: run_one(args, n), range(args.runs)))
    report = summary(args, results)
    print(report)
    out = ROOT / '.local/companion-lab' / f'report-{args.subject}-{args.scenario}{args.tag}.md'
    out.write_text(report + '\n')


if __name__ == '__main__':
    main()
