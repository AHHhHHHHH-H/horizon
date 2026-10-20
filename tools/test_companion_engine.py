#!/usr/bin/env python3
"""The engine under a companion that plays alone (docs/companion-speedrun.md, engine gaps).

In a fixed-seed world, through the control socket:

- respawn: a companion that dies with its owner far away comes back where a player would, at the world
  spawn in the Overworld (its owner elsewhere keeps their place); one that dies in the Nether away from its
  owner comes back in the Overworld, parked there until its owner returns.

    python3 tools/test_companion_engine.py --client target/fast/horizon-client --offscreen

Worlds and settings live in a temporary directory.
"""
import argparse
import json
from pathlib import Path
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_companion import command, distance, player, start, stop, wait_for  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
SEED = 20261020
WORLD = 'CompanionEngine'


def settle(endpoint, what):
    """Wait until the owner stands on loaded ground (once the move has registered)."""
    time.sleep(1)
    wait_for(endpoint, 'status', lambda v: v['player'] and v['player'].get('on_ground')
             and v['pending_chunks'] == 0, timeout=90, what=what)


def listed(endpoint, name):
    for c in command(endpoint, 'companion list')['companions']:
        if c['name'] == name:
            return c
    return None


def die_and_come_back(endpoint, name):
    command(endpoint, f'command kill {name}')
    wait_for(endpoint, 'companion list', lambda v: any(c['name'] == name and c.get('dead') for c in v['companions']),
             timeout=10, what=f'{name} dead')
    back = wait_for(endpoint, 'companion list',
                    lambda v: any(c['name'] == name and (c.get('parked') or not c['dead']) for c in v['companions']),
                    timeout=30, what=f'{name} back')
    return next(c for c in back['companions'] if c['name'] == name)


def open_world(endpoint):
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title screen')
    command(endpoint, f'open {WORLD} {SEED}')
    wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
             timeout=180, what='world loaded')
    command(endpoint, 'time 1000')
    for rule in ('advance_time', 'spawn_mobs', 'spawn_patrols', 'spawn_phantoms'):
        command(endpoint, f'command gamerule {rule} false')
    command(endpoint, 'command difficulty easy')
    command(endpoint, 'command gamemode creative')


def respawn(endpoint):
    """E6: alone, it respawns at the world spawn; in another level, it waits there for its owner."""
    spawn = player(endpoint)['world_spawn']
    assert spawn, 'the world has no spawn'
    at_spawn = [spawn[0] + 0.5, spawn[1], spawn[2] + 0.5]
    # Near the spawn, so its chunks stay loaded; the companion holds its post.
    command(endpoint, f'command tp @s {spawn[0] + 0.5} 200 {spawn[2] + 30.5}')
    settle(endpoint, 'owner near the spawn')
    command(endpoint, 'companion spawn Ari')
    command(endpoint, 'companion do Ari stay')
    time.sleep(1)
    # The owner 40 blocks off: not with it.
    command(endpoint, f'command tp @s {spawn[0] + 40.5} 200 {spawn[2] + 30.5}')
    settle(endpoint, 'owner away')
    owner = player(endpoint)['position']
    c = die_and_come_back(endpoint, 'Ari')
    assert distance(c['position'], at_spawn) < 1.5, ('not at the world spawn', c['position'], at_spawn)
    assert distance(c['position'], owner) > 20, ('beside its owner', c['position'], owner)
    print(f'respawn alone: at the world spawn {c["position"]}, owner {distance(c["position"], owner):.0f} blocks off')

    # With its owner it still comes back beside them.
    command(endpoint, 'companion do Ari follow')
    wait_for(endpoint, 'companion list', lambda v: distance(listed(endpoint, 'Ari')['position'],
                                                            player(endpoint)['position']) < 8,
             timeout=30, what='Ari with its owner')
    owner = player(endpoint)['position']
    c = die_and_come_back(endpoint, 'Ari')
    assert distance(c['position'], owner) < 8, ('not beside its owner', c['position'], owner)
    print('respawn together: beside its owner')

    # In the Nether, on the roof, away from its owner: back in the Overworld, parked there.
    command(endpoint, 'command execute in minecraft:the_nether run tp @s 0.5 128 0.5')
    wait_for(endpoint, 'status', lambda v: v['dimension'] == 'minecraft:the_nether', timeout=30, what='the Nether')
    settle(endpoint, 'owner on the Nether roof')
    wait_for(endpoint, 'companion list', lambda v: (listed(endpoint, 'Ari') or {}).get('dimension')
             == 'minecraft:the_nether', timeout=30, what='Ari came along')
    command(endpoint, 'companion do Ari stay')
    command(endpoint, 'command tp @s 40.5 128 0.5')
    settle(endpoint, 'owner away on the roof')
    c = die_and_come_back(endpoint, 'Ari')
    assert c.get('parked') == 'minecraft:overworld', ('not parked in the Overworld', c)
    # Its owner comes home: it is at the spawn.
    command(endpoint, f'command execute in minecraft:overworld run tp @s {spawn[0] + 10.5} 200 {spawn[2] + 0.5}')
    wait_for(endpoint, 'status', lambda v: v['dimension'] == 'minecraft:overworld', timeout=30, what='home')
    settle(endpoint, 'owner home')
    wait_for(endpoint, 'companion list', lambda v: (listed(endpoint, 'Ari') or {}).get('dimension')
             == 'minecraft:overworld', timeout=30, what='Ari rejoined')
    c = listed(endpoint, 'Ari')
    assert distance(c['position'], at_spawn) < 1.5, ('not at the world spawn', c, at_spawn)
    print('respawn in another level: parked in the Overworld at the spawn, rejoined when its owner came back')
    command(endpoint, 'companion dismiss Ari')


def companion_at(endpoint, name):
    c = listed(endpoint, name)
    assert c and 'position' in c, (name, c)
    return c


def block_text(endpoint, x, y, z):
    return json.dumps(command(endpoint, f'block {x} {y} {z}'))


def watching(endpoint, name, dimension):
    """The owner is the camera on `name`, in `dimension` with it."""
    status = command(endpoint, 'status')
    c = companion_at(endpoint, name)
    watch = command(endpoint, 'companion watch status')
    assert watch['watching'] == name, watch
    assert status['dimension'] == dimension and c['dimension'] == dimension, (status['dimension'], c)
    assert status['player']['game_mode'] == 3, status['player']
    assert distance(status['player']['position'], c['position']) < 8, (status['player']['position'], c)
    return c


def through(endpoint, name, dimension, timeout=60):
    wait_for(endpoint, 'status', lambda v: v['dimension'] == dimension and v['player'] is not None
             and v['pending_chunks'] == 0, timeout=timeout, what=f'the view in {dimension}')
    wait_for(endpoint, 'companion list', lambda v: (listed(endpoint, name) or {}).get('dimension') == dimension
             and 'position' in listed(endpoint, name), timeout=30, what=f'{name} in {dimension}')
    time.sleep(1)
    return watching(endpoint, name, dimension)


def watch(endpoint):
    """E1: Watch mode follows a companion through Nether and End portals; E2a: the dragon fights it."""
    spawn = player(endpoint)['world_spawn']
    command(endpoint, f'command tp @s {spawn[0] + 0.5} {spawn[1] + 2} {spawn[2] + 0.5}')
    settle(endpoint, 'owner at the spawn')
    command(endpoint, 'companion spawn Bea')
    command(endpoint, 'companion do Bea stay')
    time.sleep(1)
    for item in ('flint_and_steel', 'obsidian 10', 'iron_sword', 'bread 16'):
        command(endpoint, f'command give Bea {item}')
    c = companion_at(endpoint, 'Bea')
    x, y, z = (int(v // 1) for v in c['position'])
    # A lit Nether portal three blocks in front, on a floor, in the open.
    pz = z + 3
    command(endpoint, f'command fill {x - 3} {y} {z - 1} {x + 4} {y + 5} {z + 5} air')
    command(endpoint, f'command fill {x - 3} {y - 1} {z - 1} {x + 4} {y - 1} {z + 5} stone')
    command(endpoint, f'command fill {x - 1} {y - 1} {pz} {x + 2} {y + 3} {pz} obsidian')
    command(endpoint, f'command fill {x} {y} {pz} {x + 1} {y + 2} {pz} nether_portal[axis=x]')
    assert 'nether_portal' in block_text(endpoint, x, y, pz), block_text(endpoint, x, y, pz)
    command(endpoint, 'command difficulty peaceful')

    watched = command(endpoint, 'companion watch Bea')
    assert watched['watching'] == 'Bea' and len(watched['hud']) == 3, watched
    command(endpoint, 'companion watch step build the portal')
    command(endpoint, 'companion watch perception low')
    hud = command(endpoint, 'companion watch status')['hud']
    assert hud[1] == 'Step: build the portal' and hud[2] == 'Perception: low', hud
    watching(endpoint, 'Bea', 'minecraft:overworld')
    print(f'watch: the owner is a spectator camera on Bea; HUD {hud}')

    # By itself into the portal: the view follows it to the Nether.
    command(endpoint, f'companion do Bea goto {x} {y} {pz}')
    c = through(endpoint, 'Bea', 'minecraft:the_nether', timeout=90)
    print(f'watch: Bea walked into the portal, the view followed it to the Nether at {c["position"]}')

    # Out of its exit portal and back in: home.
    nx, ny, nz = (int(v // 1) for v in c['position'])
    exit_portal = block_text(endpoint, nx, ny, nz)
    assert 'nether_portal' in exit_portal, exit_portal
    out = (nx, ny, nz + 2) if '"x"' in exit_portal or 'axis=x' in exit_portal else (nx + 2, ny, nz)
    command(endpoint, 'companion do Bea goto {} {} {}'.format(*out))
    wait_for(endpoint, 'companion list', lambda v: 'nether_portal' not in block_text(
        endpoint, *(int(p // 1) for p in companion_at(endpoint, 'Bea')['position'])), timeout=30, what='Bea out of the portal')
    time.sleep(1)
    command(endpoint, f'companion do Bea goto {nx} {ny} {nz}')
    c = through(endpoint, 'Bea', 'minecraft:overworld', timeout=90)
    assert distance(c['position'], [x + 1, y, pz + 0.5]) < 4, ('not back at its portal', c['position'])
    print(f'watch: Bea came back through its exit portal, the view with it, at {c["position"]}')

    # Set up for the End: an End portal by the Nether one, resistance for the crystal's blast.
    for effect in ('resistance 600 4', 'slow_falling 600 0', 'fire_resistance 600 0'):
        command(endpoint, f'command effect give Bea minecraft:{effect} true')
    command(endpoint, 'companion do Bea stop')
    command(endpoint, f'command fill {x - 1} {y - 2} {z - 4} {x + 1} {y - 2} {z - 2} stone')
    command(endpoint, f'command fill {x - 1} {y - 1} {z - 4} {x + 1} {y - 1} {z - 2} end_portal')
    # Off its portal first: standing in one, its portal cooldown keeps restarting.
    command(endpoint, f'command tp Bea {x + 0.5} {y} {z + 0.5}')
    time.sleep(1)
    command(endpoint, f'command tp Bea {x + 0.5} {y - 0.5} {z - 2.5}')
    c = through(endpoint, 'Bea', 'minecraft:the_end', timeout=90)
    print(f'watch: Bea stepped onto an End portal, the view followed it to the End at {c["position"]}')

    # The fight starts from it once the arena is in (the owner only watches, a spectator): over the
    # island's centre, with the view's chunks reaching its edge.
    command(endpoint, 'render-distance 9')
    # Not peaceful: there the dragon attacks no one (LivingEntity.canAttack).
    command(endpoint, 'command difficulty easy')
    command(endpoint, 'command tp Bea 0.5 90 0.5')
    fight = wait_for(endpoint, 'status', lambda v: v['fight']['dragon'] and v['fight']['dragon']['id']
                     and all(k['id'] for k in v['fight']['crystal_list']) and v['fight']['crystal_list'],
                     timeout=60, what='the dragon')['fight']
    command(endpoint, 'companion do Bea stop')
    crystal = min(fight['crystal_list'], key=lambda k: k['position'][1])
    cx, cy, cz = crystal['position']
    command(endpoint, f'command tp Bea {cx + 1.5} {cy} {cz + 0.5}')
    time.sleep(0.5)
    bo = companion_at(endpoint, 'Bea')['entity_id']

    def broke(v):
        if not any(k['id'] == crystal['id'] for k in v['fight']['crystal_list']):
            return True
        command(endpoint, f'companion strike Bea {crystal["id"]}')
        return False
    wait_for(endpoint, 'status', broke, timeout=30, what='Bea broke a crystal')
    strafing = wait_for(endpoint, 'status', lambda v: v['fight']['dragon']['strafing'] == bo, timeout=20,
                        what='the dragon strafing Bea')['fight']['dragon']
    print(f'watch: Bea broke a crystal, the dragon strafes it (phase {strafing["phase"]}, target {bo})')

    # Its blows land: next to the dragon's body, one strike at a time.
    def hurt(v):
        d = v['fight']['dragon']
        if d['health'] < 200:
            return True
        command(endpoint, 'command tp Bea {} {} {}'.format(*d['position']))
        command(endpoint, f'companion strike Bea {d["id"] + 3}')
        return False
    dragon = wait_for(endpoint, 'status', hurt, timeout=60, what='Bea hurting the dragon')['fight']['dragon']
    assert companion_at(endpoint, 'Bea')['dimension'] == 'minecraft:the_end'
    print(f'watch: Bea hurt the dragon (health {dragon["health"]:.2f})')

    # Dead in the End, it comes back at the world spawn; the view goes home with it.
    command(endpoint, 'command kill Bea')
    c = through(endpoint, 'Bea', 'minecraft:overworld', timeout=60)
    assert distance(c['position'], [spawn[0] + 0.5, spawn[1], spawn[2] + 0.5]) < 1.5, ('not at the spawn', c, spawn)
    print(f'watch: Bea died in the End and came back at the world spawn, the view with it')

    command(endpoint, 'companion watch off')
    status = command(endpoint, 'status')
    assert status['player']['game_mode'] == 1, status['player']
    assert command(endpoint, 'companion watch status')['watching'] is None
    print('watch: off, the owner is back in creative where Bea is')
    command(endpoint, 'companion dismiss Bea')


SCENARIOS = {'respawn': respawn, 'watch': watch}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--only', choices=sorted(SCENARIOS), action='append')
    args = parser.parse_args()
    client, resources = args.client.resolve(), args.resources.resolve()
    with tempfile.TemporaryDirectory(prefix='horizon-companion-engine-') as directory:
        home = Path(directory) / 'game'
        home.mkdir()
        endpoint = home / 'control.sock'
        with (home / 'client.log').open('w+') as log:
            process = start(client, resources, home, endpoint, log, args.offscreen)
            try:
                open_world(endpoint)
                for name in args.only or list(SCENARIOS):
                    SCENARIOS[name](endpoint)
                command(endpoint, 'quit')
                assert process.wait(timeout=60) == 0, 'client exited unsuccessfully'
            except Exception:
                log.seek(0)
                print('\n'.join(log.read().splitlines()[-40:]))
                raise
            finally:
                stop(process)
    print('Companion engine passed:', ', '.join(args.only or SCENARIOS))


if __name__ == '__main__':
    main()
