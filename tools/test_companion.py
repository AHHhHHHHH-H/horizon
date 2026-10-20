#!/usr/bin/env python3
"""Companion body end to end (docs/companions.md): in a fixed-seed world, a
companion spawns beside the player, follows them across the terrain (and
teleports when left far behind), picks up and equips a sword and eats,
fights a zombie, dies (dropping what it carried), respawns beside its owner
after 10 s, is saved with the world and comes back when it is reopened.
It also measures what the companion costs a tick and checks the frame cost.

    python3 tools/test_companion.py --client target/fast/horizon-client [--offscreen]

Linux needs a display (xvfb-run -a); macOS can pass --offscreen. Worlds and
settings live in a temporary directory; --shots keeps screenshots.
"""
import argparse
import json
import math
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
SEED = 20261020
WORLD = 'CompanionTest'


def command(endpoint, line, timeout=120):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(timeout)
        connection.connect(str(endpoint))
        connection.sendall((line + '\n').encode())
        with connection.makefile('r') as reply:
            result = json.loads(reply.readline())
    assert 'error' not in result, (line, result)
    return result


def wait_for(endpoint, line, predicate, timeout=60, what=None):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            last = command(endpoint, line)
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.2)
            continue
        if predicate(last):
            return last
        time.sleep(0.2)
    raise AssertionError(f'timed out waiting for {what or line}: {last}')


def companion(endpoint):
    listed = command(endpoint, 'companion list')['companions']
    return listed[0] if listed else None


def player(endpoint):
    return command(endpoint, 'status')['player']


def distance(a, b):
    return math.dist(a, b)


def reports(endpoint, seen):
    """Every report since the last call, appended to `seen`."""
    new = [r['text'] for r in command(endpoint, 'companion reports')['reports']]
    seen.extend(new)
    return new


SOLID_NOT = ('air', 'water', 'lava', 'grass', 'fern', 'flower', 'leaves', 'snow', 'torch', 'carpet')


def firm_ground(endpoint, at):
    """A spot 3 or 4 blocks from `at`, on two solid blocks with room above:
    a mob summoned there does not fall into a well or a cave."""
    x, y, z = (math.floor(v) for v in at)
    def name(dx, dy, dz):
        return command(endpoint, f'block {x + dx} {y + dy} {z + dz}')['block']
    for dx, dz in ((3, 0), (-3, 0), (0, 3), (0, -3), (3, 3), (-3, -3), (3, -3), (-3, 3), (4, 0), (-4, 0)):
        for dy in (0, 1, -1):
            ground = [name(dx, dy - 1, dz), name(dx, dy - 2, dz)]
            room = [name(dx, dy, dz), name(dx, dy + 1, dz)]
            if all(not any(w in g for w in SOLID_NOT) for g in ground) and all(r.endswith(':air') for r in room):
                return [x + dx + 0.5, y + dy, z + dz + 0.5]
    return [at[0] + 3, at[1], at[2]]


def near_owner(endpoint, limit):
    def ok(listed):
        c = listed['companions'][0] if listed['companions'] else None
        p = player(endpoint)
        return c and not c['dead'] and p and distance(c['position'], p['position']) <= limit
    return ok


def look_at(endpoint, target):
    p = player(endpoint)['position']
    eye = [p[0], p[1] + 1.62, p[2]]
    d = [target[0] - eye[0], target[1] + 1.0 - eye[1], target[2] - eye[2]]
    yaw = math.degrees(math.atan2(-d[0], d[2]))
    pitch = -math.degrees(math.atan2(d[1], math.hypot(d[0], d[2])))
    command(endpoint, f'look {yaw:.2f} {pitch:.2f}')


def scenario(endpoint, shots, home):
    log = []
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title screen')
    command(endpoint, f'open {WORLD} {SEED}')
    wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
             timeout=180, what='world loaded')
    command(endpoint, 'time 1000')
    # 26.x rule names: no clock, no natural spawns, patrols or phantoms.
    for rule in ('advance_time', 'spawn_mobs', 'spawn_patrols', 'spawn_phantoms'):
        out = command(endpoint, f'command gamerule {rule} false')
        assert out['chat'][0] == f'Game rule {rule} is now set to false', (rule, out)
    command(endpoint, 'command difficulty easy')

    # The same ground every run: vanilla's spawn point moves within the
    # spawn radius, so the owner lands on a fixed column first.
    command(endpoint, 'command gamemode creative')
    command(endpoint, 'command tp @s 0.5 200 0.5')
    wait_for(endpoint, 'status', lambda v: v['player'] and v['player'].get('on_ground')
             and v['pending_chunks'] == 0, timeout=60, what='owner at the start')
    command(endpoint, 'command gamemode survival')

    # Nothing runs before a companion is spawned: no hub.
    assert command(endpoint, 'lan status') == {'open': False}
    assert command(endpoint, 'companion list')['companions'] == []

    spawned = command(endpoint, 'companion spawn Ari')
    assert spawned['name'] == 'Ari', spawned
    c = companion(endpoint)
    me = player(endpoint)['position']
    # Beside the owner, or on the nearest floor below a tree top.
    assert distance(c['position'], me) < 8, (c, me)
    assert c['game_mode'] == 0 and c['health'] == 20, c
    # A single-player world stays closed to LAN (and pauses) with a companion.
    assert command(endpoint, 'lan status') == {'open': False}
    print(f'spawned {c["name"]} wearing {c["skin"]} at {[round(v, 1) for v in c["position"]]}')

    # Follow across the terrain: the owner lands 30 blocks away (closer than
    # the teleport distance), then sprints on; the companion walks after them.
    command(endpoint, 'command gamemode creative')
    command(endpoint, f'command tp @s {me[0] + 30:.1f} 200 {me[2] + 12:.1f}')
    wait_for(endpoint, 'status', lambda v: v['player'] and v['player'].get('on_ground')
             and v['pending_chunks'] == 0, timeout=60, what='owner landed')
    command(endpoint, 'command gamemode survival')
    walked = player(endpoint)['position']
    print(f'owner went {distance(me, walked):.1f} blocks away')
    wait_for(endpoint, 'companion list', near_owner(endpoint, 7), timeout=60, what='follow')
    reports(endpoint, log)
    assert 'teleported to owner' not in log, (log, companion(endpoint), player(endpoint))
    for yaw in (0, 90):
        command(endpoint, f'look {yaw} 0')
        command(endpoint, 'move forward 3 sprint jump', timeout=30)
    wait_for(endpoint, 'companion list', near_owner(endpoint, 7), timeout=40, what='follow')
    reports(endpoint, log)
    cost_follow = command(endpoint, 'companion cost')['cost']
    print(f'followed: now {distance(companion(endpoint)["position"], player(endpoint)["position"]):.1f} '
          f"blocks from its owner; hub tick {cost_follow['mean_ms']:.3f} ms mean ({cost_follow})")

    if shots:
        c = companion(endpoint)
        look_at(endpoint, c['position'])
        time.sleep(0.5)
        command(endpoint, f'screenshot {shots / "companion-follow.png"}')

    # Teleport: the owner goes 80 blocks away (creative, to land safely)
    # while it stays put (it would otherwise run after them as they fall).
    command(endpoint, 'companion do stay')
    command(endpoint, 'command gamemode creative')
    here = player(endpoint)['position']
    command(endpoint, f'command tp @s {here[0] + 80:.1f} 200 {here[2]:.1f}')
    wait_for(endpoint, 'status', lambda v: v['player'] and v['player'].get('on_ground')
             and v['pending_chunks'] == 0, timeout=60, what='owner landed')
    command(endpoint, 'companion do follow')
    wait_for(endpoint, 'companion list', near_owner(endpoint, 6), timeout=30, what='teleport to owner')
    reports(endpoint, log)
    assert any('teleported' in r for r in log), log
    command(endpoint, 'command gamemode survival')
    print('left 80 blocks behind: teleported beside its owner')

    # Pickups and gear: the owner drops a sword, bread and a helmet near it.
    for item in ('iron_sword', 'bread 4', 'iron_helmet'):
        command(endpoint, f'give {item}')
        command(endpoint, 'select 1')
        # At its feet, on the ground the owner stands on.
        command(endpoint, 'look 0 60')
        # The given item is in the first free slot: hold and drop each.
        inventory = command(endpoint, 'inventory')
        slot = next(i for i, s in enumerate(inventory['hotbar']) if s and s.get('item'))
        command(endpoint, f'select {slot + 1}')
        command(endpoint, 'drop all')
    # The owner steps back: what lies there is the companion's to pick up.
    command(endpoint, 'move back 0.5', timeout=30)
    try:
        wait_for(endpoint, 'companion list', lambda v: v['companions'][0]['items'] >= 5
                 and v['companions'][0]['held'] == 'minecraft:iron_sword'
                 and v['companions'][0]['armor'][0] == 'minecraft:iron_helmet',
                 timeout=40, what='pickups and gear')
    except AssertionError:
        print('owner', player(endpoint), 'drops', command(endpoint, 'entities 24 item'))
        raise
    reports(endpoint, log)
    assert any('picked up' in r for r in log) and any('equipped iron_helmet' in r for r in log), log
    print('picked up the sword, bread and helmet; wears the helmet, holds the sword')

    # Eating below 14 food.
    command(endpoint, 'command effect give Ari hunger 30 255')
    wait_for(endpoint, 'companion list', lambda v: any('ate bread' in r for r in reports(endpoint, log)),
             timeout=60, what='eating')
    print('ate bread when hungry')
    command(endpoint, 'command effect clear Ari')

    # Fight: night, a zombie next to it, attack nearest.
    command(endpoint, 'time 18000')
    c = companion(endpoint)
    at = c['position']
    spot = firm_ground(endpoint, at)
    zombie = command(endpoint, f'summon zombie {spot[0]:.1f} {spot[1]:.1f} {spot[2]:.1f}')['id']
    command(endpoint, 'companion cost')
    command(endpoint, 'companion do attack nearest')
    wait_for(endpoint, 'mobs 64', lambda v: not any(m['id'] == zombie and m['health'] > 0
                                                    for m in v['near']),
             timeout=60, what='zombie dead')
    cost_fight = command(endpoint, 'companion cost')['cost']
    reports(endpoint, log)
    print(f'killed the zombie; hub tick while fighting {cost_fight["mean_ms"]:.3f} ms mean, '
          f'{cost_fight["max_ms"]:.3f} ms worst')
    command(endpoint, 'time 1000')

    if shots:
        c = companion(endpoint)
        look_at(endpoint, c['position'])
        time.sleep(0.5)
        command(endpoint, f'screenshot {shots / "companion-armed.png"}')

    # Death: its items drop where it fell, it comes back beside its owner in 10 s.
    command(endpoint, 'command kill Ari')
    dead = wait_for(endpoint, 'companion list', lambda v: v['companions'][0]['dead'], timeout=10,
                    what='death')['companions'][0]
    assert dead['items'] == 0, dead
    drops = command(endpoint, 'entities 32 item')['entities']
    assert any(e['item']['item'] == 'minecraft:iron_sword' for e in drops), drops
    died = time.monotonic()
    wait_for(endpoint, 'companion list', lambda v: not v['companions'][0]['dead'], timeout=20,
             what='respawn')
    waited = time.monotonic() - died
    c = companion(endpoint)
    assert distance(c['position'], player(endpoint)['position']) < 5, c
    assert c['health'] == 20, c
    reports(endpoint, log)
    assert any(r.startswith('died') for r in log) and 'respawned' in log, log
    print(f'died, dropped its things, respawned beside its owner after {waited:.1f} s')

    # Save and reload: it comes back with the world, where it stood.
    command(endpoint, 'companion do stay')
    time.sleep(1)
    before = companion(endpoint)
    command(endpoint, 'key esc tap')
    wait_for(endpoint, 'screen', lambda v: any(b['label'] == 'Save and Quit to Title'
                                               for b in v.get('buttons', [])), what='pause menu')
    command(endpoint, 'click-button Save and Quit to Title')
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', what='title')
    assert command(endpoint, 'lan status') == {'open': False}
    files = list((home / 'saves' / WORLD / 'companions').glob('*.json'))
    assert len(files) == 1, files
    saved = json.loads(files[0].read_text())
    assert saved['name'] == 'Ari' and saved['active'], saved
    assert (home / 'saves' / WORLD / 'players' / 'data' / f'{saved["uuid"]}.dat').is_file()
    command(endpoint, f'open {WORLD}')
    back = wait_for(endpoint, 'companion list', lambda v: v['companions'] and 'position' in v['companions'][0],
                    timeout=120, what='companion back')['companions'][0]
    assert back['uuid'] == before['uuid'], (back, before)
    assert distance(back['position'], before['position']) < 1.0, (back, before)
    assert back['skill'] == 'stay', back
    print(f'reloaded with the world at {[round(v, 1) for v in back["position"]]}')

    # Dismissed, the hub it needed goes too.
    command(endpoint, 'companion dismiss')
    assert command(endpoint, 'companion list')['companions'] == []
    assert command(endpoint, 'lan status') == {'open': False}
    saved = json.loads(files[0].read_text())
    assert not saved['active'], saved
    command(endpoint, 'quit')
    return cost_follow, cost_fight, log


def frame_cost(client, resources, work, offscreen):
    """Frame time over 8 s looking at the same spot, without and with a companion."""
    home = work / 'frames'
    home.mkdir()
    endpoint = home / 'control.sock'
    with (home / 'client.log').open('w+') as log:
        process = start(client, resources, home, endpoint, log, offscreen)
        try:
            wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title')
            command(endpoint, f'open Frames {SEED}')
            wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
                     timeout=180)
            command(endpoint, 'time 1000')
            command(endpoint, 'command gamerule advance_time false')
            command(endpoint, 'look 0 10')
            time.sleep(3)
            without = command(endpoint, 'bench 8', timeout=30)
            command(endpoint, 'companion spawn Kai')
            command(endpoint, 'companion do stay')
            c = companion(endpoint)
            look_at(endpoint, c['position'])
            time.sleep(3)
            with_one = command(endpoint, 'bench 8', timeout=30)
            command(endpoint, 'companion dismiss')
            time.sleep(1)
            after = command(endpoint, 'bench 8', timeout=30)
            command(endpoint, 'quit')
            process.wait(timeout=30)
        finally:
            stop(process)
    return without, with_one, after


def start(client, resources, home, endpoint, log, offscreen):
    extra = ['--offscreen'] if offscreen else []
    return subprocess.Popen([
        str(client), '--resources', str(resources), '--render-distance', '6',
        '--control-socket', str(endpoint), '--timeout-seconds', '900', *extra,
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--shots', type=Path, help='keep screenshots in this folder')
    parser.add_argument('--frames', action='store_true', help='also compare frame times')
    args = parser.parse_args()
    client, resources = args.client.resolve(), args.resources.resolve()
    shots = args.shots.resolve() if args.shots else None
    if shots:
        shots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='horizon-companion-') as directory:
        work = Path(directory)
        home = work / 'game'
        home.mkdir()
        endpoint = home / 'control.sock'
        with (home / 'client.log').open('w+') as log:
            process = start(client, resources, home, endpoint, log, args.offscreen)
            try:
                cost_follow, cost_fight, events = scenario(endpoint, shots, home)
                assert process.wait(timeout=30) == 0, 'client exited unsuccessfully'
            except Exception:
                log.seek(0)
                print('\n'.join(log.read().splitlines()[-40:]))
                raise
            finally:
                stop(process)
        print('reports:', '; '.join(events[-25:]))
        print(f'companion tick cost: following {cost_follow["mean_ms"]:.3f} ms mean '
              f'({cost_follow["max_ms"]:.3f} worst), fighting {cost_fight["mean_ms"]:.3f} ms mean '
              f'({cost_fight["max_ms"]:.3f} worst)')
        if args.frames:
            without, with_one, after = frame_cost(client, resources, work, args.offscreen)
            for name, b in (('no companion', without), ('one companion in view', with_one),
                            ('dismissed', after)):
                print(f'frames, {name}: {json.dumps(b)}')
    print('Companion body passed: spawn, follow, teleport, pickups, gear, eating, '
          'fight, death and respawn, save and reload, dismiss')


if __name__ == '__main__':
    main()
