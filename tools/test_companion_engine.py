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
    """Wait until the owner stands on loaded ground."""
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


SCENARIOS = {'respawn': respawn}


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
