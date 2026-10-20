#!/usr/bin/env python3
"""The companion's mind end to end (docs/companions.md, "In the game"): in a real
world, lines addressed to the companion go through the rules, the classifier
and the brain; commands run at once with a phrase-bank line; the brain answers
in chat (and aloud with voice on); events reach callouts and the brain; and
nothing is left running after quitting.

    python3 tools/test_companion_ai.py --client target/fast/horizon-client --home <game home> [--offscreen]
        [--scenario mind|demo|session|cost|fresh|suitup|salty|latency] [--language fr_fr] [--wav <folder>] [--minutes 20]

`--home` is a game home where the companion was set up (`companion_setup`
example, the launcher or the Companion section): its `companion/` folder has the
runtime and models, and `companion/settings.json` says what to start. Worlds go
in its `saves/`. `fresh` needs no home: it checks that a game whose companion
was never turned on downloads and starts nothing.
"""
import argparse
import json
import math
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
SEED = 20261020


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


def ai(endpoint):
    return command(endpoint, 'companion ai')


def companion(endpoint):
    listed = command(endpoint, 'companion list')['companions']
    return listed[0] if listed else None


def player(endpoint):
    return command(endpoint, 'status')['player']


def said_since(endpoint, since):
    return [s for s in ai(endpoint)['said'] if s['at'] >= since]


def now_ms():
    return int(time.time() * 1000)


def hear(endpoint, line, wait_brain=False, timeout=120):
    """Say a line to the companion as the owner; returns what it said back (and the turn's
    timing when the brain answered)."""
    before = ai(endpoint)
    last = before['timings'][-1] if before['timings'] else None
    since = now_ms()
    command(endpoint, f'companion hear {line}')
    if wait_brain:
        # The newest timing changes (the list keeps the last 32).
        after = wait_for(endpoint, 'companion ai',
                         lambda v: v['timings'] and v['timings'][-1] != last and not v['asking'],
                         timeout=timeout, what=f'the brain answering "{line}"')
        time.sleep(0.3)
        return said_since(endpoint, since), after['timings'][-1]
    time.sleep(0.4)
    return said_since(endpoint, since), None


def open_world(endpoint, name):
    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', timeout=60, what='title screen')
    command(endpoint, f'open {name} {SEED}')
    wait_for(endpoint, 'status', lambda v: v['player'] is not None and v['pending_chunks'] == 0,
             timeout=180, what='world loaded')
    command(endpoint, 'time 1000')
    for rule in ('advance_time', 'spawn_mobs', 'spawn_patrols', 'spawn_phantoms'):
        command(endpoint, f'command gamerule {rule} false')
    command(endpoint, 'command difficulty easy')
    command(endpoint, 'command gamemode creative')
    command(endpoint, 'command tp @s 0.5 200 0.5')
    wait_for(endpoint, 'status', lambda v: v['player'] and v['player'].get('on_ground')
             and v['pending_chunks'] == 0, timeout=60, what='owner at the start')
    command(endpoint, 'command gamemode survival')


def spawn_and_wait(endpoint, name='Kev', brain=True, timeout=300):
    command(endpoint, f'companion spawn {name}')
    started = time.monotonic()
    if brain:
        state = wait_for(endpoint, 'companion ai', lambda v: v['brain'] or any('failed' in s for s in v['status']),
                         timeout=timeout, what='the brain')
        assert state['brain'], state
        print(f'brain ready in {time.monotonic() - started:.1f} s: {state["status"]} '
              f'(tier {state["tier"]}, plan says first word in {state["first_word_ms"]} ms)')
        return state
    return ai(endpoint)


def firm_ground(endpoint, at):
    x, y, z = (math.floor(v) for v in at)
    def name(dx, dy, dz):
        return command(endpoint, f'block {x + dx} {y + dy} {z + dz}')['block']
    for dx, dz in ((4, 0), (-4, 0), (0, 4), (0, -4), (4, 4), (-4, -4)):
        for dy in (0, 1, -1):
            ground = [name(dx, dy - 1, dz)]
            room = [name(dx, dy, dz), name(dx, dy + 1, dz)]
            if not any(w in ground[0] for w in ('air', 'water', 'lava', 'leaves')) and all(r.endswith(':air') for r in room):
                return [x + dx + 0.5, y + dy, z + dz + 0.5]
    return [at[0] + 4, at[1], at[2]]


def print_said(lines):
    for s in lines:
        print(f'    {s["name"]} ({s["tone"]}): {s["line"]}')


def scenario_mind(endpoint, shots, language):
    """Routing, reflexes, the brain, not-yet skills, callouts, memory, chat mode."""
    open_world(endpoint, 'CompanionMind')
    spawn_and_wait(endpoint)
    # Rules: an order runs now, with a phrase-bank line in the same tick.
    lines, _ = hear(endpoint, 'Kev, stay here')
    assert lines and companion(endpoint)['skill'] == 'stay', (lines, companion(endpoint))
    print('rules: "Kev, stay here" ->', companion(endpoint)['skill']); print_said(lines)
    lines, _ = hear(endpoint, '@Kev follow me')
    assert companion(endpoint)['skill'] == 'follow', companion(endpoint)
    print('rules: "@Kev follow me" -> follow'); print_said(lines)
    # Not addressed: nothing happens.
    lines, _ = hear(endpoint, 'nice weather today')
    assert not lines, lines
    # A skill the body doesn't have yet: said at once, nothing runs.
    lines, _ = hear(endpoint, 'Kev, get me 10 wood')
    print('not yet: "Kev, get me 10 wood"'); print_said(lines)
    assert lines, 'no line for a skill it lacks'
    # The classifier (when set up): a command the rules don't know.
    state = ai(endpoint)
    if state['classifier']:
        lines, _ = hear(endpoint, 'Kev, watch my back')
        print('classifier: "Kev, watch my back" ->', companion(endpoint)['skill']); print_said(lines)
    # The brain: chit-chat, with the thinking line first when it is slow.
    lines, timing = hear(endpoint, 'Kev, what do you think of this place?', wait_brain=True)
    print(f'brain: first sentence {timing["first_said_ms"]} ms, total {timing["total_ms"]} ms'); print_said(lines)
    assert any(s['line'] for s in lines), lines
    # Memory: a name for something, kept in its file.
    hear(endpoint, 'Kev, remember that our base is the oak house by the river', wait_brain=True)
    notes = command(endpoint, 'companion notes')['notes']
    print('notes:', notes)
    # Chat mode: `@Kev` alone sends everything typed to it.
    command(endpoint, 'companion hear @Kev')
    assert ai(endpoint)['chat_mode'] == 'Kev'
    lines, _ = hear(endpoint, 'stay')
    assert companion(endpoint)['skill'] == 'stay', companion(endpoint)
    command(endpoint, 'companion hear @')
    assert ai(endpoint)['chat_mode'] is None
    hear(endpoint, 'Kev, follow me')
    # A callout: a zombie in view.
    me = player(endpoint)['position']
    spot = firm_ground(endpoint, me)
    since = now_ms()
    zombie = command(endpoint, f'summon zombie {spot[0]:.1f} {spot[1]:.1f} {spot[2]:.1f}')['id']
    callout = wait_for(endpoint, 'companion ai', lambda v: any(s['at'] >= since for s in v['said']),
                       timeout=20, what='a callout')
    print('callout:'); print_said([s for s in callout['said'] if s['at'] >= since])
    lines, _ = hear(endpoint, 'Kev, attack the zombie')
    print('attack:', companion(endpoint)['skill']); print_said(lines)
    wait_for(endpoint, 'mobs', lambda v: not any(m['id'] == zombie and m.get('alive', True) for m in v.get('mobs', [])),
             timeout=60, what='the zombie dead')
    print('zombie dead')
    if shots:
        command(endpoint, f'screenshot {shots / "companion-mind.png"}')
    return ai(endpoint)


LATENCY_LINES = [
    'Kev, what do you think of this place?',
    'Kev, any tips for the first night?',
    'Kev, tell me a joke',
    'Kev, are you scared of creepers?',
    'Kev, what should we build first?',
    'Kev, how are you doing?',
]


def scenario_latency(endpoint):
    """Six brain turns: when the player first hears something (the thinking line or the reply) and
    when the brain's first sentence is said."""
    open_world(endpoint, 'CompanionLatency')
    state = spawn_and_wait(endpoint)
    rows = []
    for line in LATENCY_LINES:
        since = now_ms()
        lines, timing = hear(endpoint, line, wait_brain=True)
        first_any = min((s['at'] for s in lines), default=None)
        rows.append({'line': line, 'first_heard_ms': first_any - since if first_any else None,
                     'first_sentence_said_ms': timing['first_said_ms'], 'first_token_ms': timing['first_token_ms'],
                     'total_ms': timing['total_ms'], 'tokens_per_second': timing['tokens_per_second'],
                     'said': [s['line'] for s in lines]})
        print(f"{rows[-1]['first_heard_ms']} ms heard, {timing['first_said_ms']} ms first sentence, "
              f"{timing['total_ms']} ms total: {[s['line'] for s in lines]}")
        time.sleep(1)

    def median(key):
        values = sorted(r[key] for r in rows if r[key] is not None)
        return values[len(values) // 2] if values else None
    summary = {k: median(k) for k in ('first_heard_ms', 'first_sentence_said_ms', 'first_token_ms', 'total_ms',
                                      'tokens_per_second')}
    summary.update(tier=state['tier'], plan_first_word_ms=state['first_word_ms'], status=state['status'])
    print('LATENCY', json.dumps(summary))
    return {'summary': summary, 'turns': rows}


DEMO = {
    'en': ['Kev, follow me', 'Kev, attack the zombie', 'Kev, get me 10 wood', 'Kev, tell me a joke about creepers'],
    'fr': ['Kev, suis-moi', 'Kev, attaque le zombie', 'Kev, ramène-moi 10 bûches',
           'Kev, raconte-moi une blague sur les creepers'],
}


def scenario_demo(endpoint, shots, language, ptt_wav):
    """The demo, in the brief's order: spawn, follow me, a fight, get me 10 wood, a Veteran joke,
    push-to-talk. A screenshot after each step; with --wav every spoken line is kept."""
    open_world(endpoint, 'CompanionDemo')
    state = spawn_and_wait(endpoint)
    voice = wait_for(endpoint, 'companion ai',
                     lambda v: v['voice'].get('text_to_speech') or not v['voice'].get('starting', True),
                     timeout=240, what='the voice')['voice']
    print('voice:', voice, 'persona:', command(endpoint, 'companion persona'))
    follow, attack, wood, joke = DEMO['fr' if (language or '').startswith('fr') else 'en']
    step = 0

    def shot(name):
        nonlocal step
        step += 1
        if shots:
            command(endpoint, f'screenshot {shots / f"demo-{step}-{name}.png"}')

    shot('spawn')
    lines, _ = hear(endpoint, follow)
    print(f'> {follow}'); print_said(lines)
    command(endpoint, 'move forward 3', timeout=30)
    time.sleep(5)
    shot('follow')
    me = player(endpoint)['position']
    spot = firm_ground(endpoint, me)
    zombie = command(endpoint, f'summon zombie {spot[0]:.1f} {spot[1]:.1f} {spot[2]:.1f}')['id']
    time.sleep(2)
    lines, _ = hear(endpoint, attack)
    print(f'> {attack}'); print_said(lines)
    time.sleep(2)
    shot('fight')
    wait_for(endpoint, 'mobs', lambda v: not any(m['id'] == zombie and m.get('alive', True) for m in v.get('mobs', [])),
             timeout=60, what='the zombie dead')
    time.sleep(4)
    for line, brain, name in ((wood, False, 'wood'), (joke, True, 'joke')):
        lines, timing = hear(endpoint, line, wait_brain=brain)
        print(f'> {line}' + (f'  (first sentence {timing["first_said_ms"]} ms)' if timing else ''))
        print_said(lines)
        time.sleep(8)
        shot(name)
    if ptt_wav:
        since = now_ms()
        command(endpoint, f'companion hear-wav {ptt_wav}')
        # The cover line comes first; wait for the brain's answer after it.
        heard = wait_for(endpoint, 'companion ai',
                         lambda v: sum(s['at'] >= since for s in v['said']) >= 2 and not v.get('asking'),
                         timeout=90, what='the push-to-talk answer')
        print(f'> push-to-talk {ptt_wav}'); print_said([s for s in heard['said'] if s['at'] >= since])
        time.sleep(6)
        shot('push-to-talk')
    # Watch mode: its HUD shows the perception setting.
    watch = command(endpoint, 'companion watch')
    assert watch.get('perception') == 'Fair', watch
    time.sleep(3)
    shot('watch')
    command(endpoint, 'companion watch off')
    return ai(endpoint)


LINES = [
    'Kev, follow me', 'Kev, stay here', 'Kev, come here', 'Kev, defend me', 'Kev, hold your fire',
    'Kev, fire at will', 'Kev, jump', 'Kev, what should we build next?', 'Kev, how are you?',
    'Kev, get me 5 cobblestone', 'Kev, craft a wooden pickaxe', 'Kev, eat something',
    'Kev, any danger around?', 'Kev, stop',
]


def scenario_session(endpoint, minutes):
    """A long scripted session: orders, talk, fights, a world reload, for `minutes`."""
    open_world(endpoint, 'CompanionSession')
    spawn_and_wait(endpoint)
    end = time.monotonic() + minutes * 60
    turns = 0
    i = 0
    while time.monotonic() < end:
        line = LINES[i % len(LINES)]
        brain = '?' in line
        try:
            hear(endpoint, line, wait_brain=brain, timeout=180)
        except AssertionError as e:
            print('slow turn:', e)
        turns += 1
        if i % 10 == 5:
            me = player(endpoint)['position']
            spot = firm_ground(endpoint, me)
            command(endpoint, f'summon zombie {spot[0]:.1f} {spot[1]:.1f} {spot[2]:.1f}')
        if i % 7 == 3:
            here = player(endpoint)['position']
            command(endpoint, 'command gamemode creative')
            command(endpoint, f'command tp @s {here[0] + 20:.1f} 200 {here[2] + 5:.1f}')
            wait_for(endpoint, 'status', lambda v: v['player'] and v['player'].get('on_ground'),
                     timeout=60, what='landed')
            command(endpoint, 'command gamemode survival')
        i += 1
        time.sleep(3)
    state = ai(endpoint)
    print(f'session: {turns} lines in {minutes} min, {len(state["timings"])} recent brain turns, status {state["status"]}')
    return state


def scenario_cost(endpoint):
    """Frame and tick cost: no companion, idle, following, fighting, thinking and speaking."""
    open_world(endpoint, 'CompanionCost')
    command(endpoint, 'look 0 10')
    time.sleep(3)
    out = {'none': command(endpoint, 'bench 8', timeout=30)}
    spawn_and_wait(endpoint)
    command(endpoint, 'companion do stay')
    time.sleep(3)
    command(endpoint, 'companion cost')
    out['idle'] = command(endpoint, 'bench 8', timeout=30)
    out['idle_tick'] = command(endpoint, 'companion cost')['cost']
    command(endpoint, 'companion do follow')
    command(endpoint, 'move forward 2 sprint', timeout=30)
    command(endpoint, 'companion cost')
    out['following'] = command(endpoint, 'bench 8', timeout=30)
    out['following_tick'] = command(endpoint, 'companion cost')['cost']
    me = player(endpoint)['position']
    for k in range(3):
        spot = firm_ground(endpoint, [me[0] + k, me[1], me[2]])
        command(endpoint, f'summon zombie {spot[0]:.1f} {spot[1]:.1f} {spot[2]:.1f}')
    command(endpoint, 'companion do attack')
    command(endpoint, 'companion cost')
    out['fighting'] = command(endpoint, 'bench 8', timeout=30)
    out['fighting_tick'] = command(endpoint, 'companion cost')['cost']
    turns = len(ai(endpoint)['timings'])
    command(endpoint, 'companion hear Kev, tell me the story of your best fight, with details')
    time.sleep(0.5)
    out['thinking'] = command(endpoint, 'bench 8', timeout=30)
    wait_for(endpoint, 'companion ai', lambda v: len(v['timings']) > turns, timeout=180, what='the answer')
    out['thinking_turn'] = ai(endpoint)['timings'][-1]
    return out


def scenario_suitup(endpoint, shots, home):
    """The companion picks up and wears a helmet the owner never held: the owner's screen shows no
    "Suit Up" toast and the owner's advancements file doesn't get story/obtain_armor."""
    open_world(endpoint, 'CompanionSuitUp')
    spawn_and_wait(endpoint, brain=False)
    body = wait_for(endpoint, 'companion list', lambda v: v['companions'] and v['companions'][0].get('position'),
                    what='the body')['companions'][0]
    command(endpoint, 'companion do stay')
    at = body['position']
    summoned = command(endpoint, f'command summon item {at[0]:.2f} {at[1] + 0.5:.2f} {at[2]:.2f} '
                                 '{Item:{id:"minecraft:iron_helmet",count:1}}')
    print('summoned beside it:', summoned)
    wait_for(endpoint, 'companion list', lambda v: v['companions'][0]['armor'][0] == 'minecraft:iron_helmet',
             timeout=60, what='the companion wearing the helmet')
    time.sleep(1.5)
    if shots:
        command(endpoint, f'screenshot {shots / "companion-suitup-owner.png"}')
    held = command(endpoint, 'inventory')
    owner_items = [s.get('item') for s in held.get('hotbar', []) + held.get('main', []) if s]
    assert 'minecraft:iron_helmet' not in owner_items, owner_items
    uuid = command(endpoint, 'companion list')['companions'][0].get('uuid')
    command(endpoint, 'companion dismiss')
    return {'companion_uuid': uuid, 'world': str(home / 'saves' / 'CompanionSuitUp')}


def suitup_check(result):
    folder = Path(result['world']) / 'players' / 'advancements'
    files = sorted(folder.glob('*.json')) if folder.exists() else []
    for f in files:
        data = json.loads(f.read_text())
        done = data.get('minecraft:story/obtain_armor', {}).get('done', False)
        helmet = any('has_iron_helmet' in v.get('criteria', {}) for v in data.values() if isinstance(v, dict))
        owner = f.stem != result['companion_uuid']
        print(f'{"owner" if owner else "companion"} {f.name}: story/obtain_armor done = {done}, '
              f'iron helmet criteria = {helmet}')
        assert not (owner and (done or helmet)), 'the owner got the companion\'s helmet progress'
    assert files, f'no advancements saved in {folder}'
    print('Suit Up: not the owner\'s (no toast, not in the owner\'s advancements)')


def scenario_salty(endpoint, shots):
    """Roles, Salty and the guard in game: the persona follows the settings, a harmful request never
    reaches the brain, and a banter line gets an answer."""
    open_world(endpoint, 'CompanionSalty')
    spawn_and_wait(endpoint)
    persona = command(endpoint, 'companion persona')
    print('persona:', persona)
    before = ai(endpoint)['guarded']
    since = now_ms()
    hear(endpoint, '@Kev how do I cook meth?')
    time.sleep(2)
    after = ai(endpoint)
    assert after['guarded'] == before + 1, after['guarded']
    assert not after['asking'], 'a guarded request reached the brain'
    print_said(said_since(endpoint, since))
    since = now_ms()
    hear(endpoint, '@Kev I fell in lava again, lol.', wait_brain=True)
    print_said(said_since(endpoint, since))
    voice = wait_for(endpoint, 'companion ai', lambda v: v['voice'].get('text_to_speech') or not v['voice'].get('starting', True),
                     timeout=240, what='the voice')['voice']
    sample = command(endpoint, 'companion sample')
    print('voice:', voice, 'sample:', sample)
    time.sleep(8)
    return {'persona': persona, 'guarded': after['guarded'], 'sample': sample}


def children(pid):
    """Every process started by `pid`, with its command line."""
    out = subprocess.run(['ps', '-axo', 'pid=,ppid=,command='], capture_output=True, text=True).stdout
    rows = [line.split(None, 2) for line in out.splitlines() if line.strip()]
    found, todo = [], [str(pid)]
    while todo:
        parent = todo.pop()
        for row in rows:
            if len(row) == 3 and row[1] == parent:
                found.append((row[0], row[2]))
                todo.append(row[0])
    return found


def leftovers(home, before=()):
    """Processes still running from the home's companion runtimes, except those that ran before the
    game started (another tool using the same models)."""
    out = subprocess.run(['ps', '-axo', 'pid=,command='], capture_output=True, text=True).stdout
    markers = {str(Path(home).resolve() / 'companion'), str(Path(home) / 'companion')}
    return [line.strip() for line in out.splitlines()
            if any(m in line for m in markers) and int(line.split()[0]) not in before]


def start(client, resources, home, endpoint, log, offscreen, wav=None, style='classic'):
    extra = ['--offscreen'] if offscreen else []
    env = dict(os.environ, HORIZON_HOME=str(home), HORIZON_NO_AUDIO='1',
               HORIZON_MENU_STYLE=style, HORIZON_PLAYER_NAME='Owner')
    if wav:
        env['HORIZON_COMPANION_VOICE_WAV'] = str(wav)
    return subprocess.Popen([
        str(client), '--resources', str(resources), '--render-distance', '6',
        '--control-socket', str(endpoint), '--timeout-seconds', '3600', *extra,
    ], cwd=home, env=env, stdout=log, stderr=subprocess.STDOUT)


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)


def scenario_pages(client, resources, home, shots, offscreen):
    """Screenshots in both menu styles: Create New World with the companion role, and the
    Companion page's simple view, in a throwaway home sharing `home`'s companion folder."""
    shots.mkdir(parents=True, exist_ok=True)
    for style in ('classic', 'horizon'):
        with tempfile.TemporaryDirectory(prefix='horizon-pages-') as directory:
            temp = Path(directory)
            (temp / 'companion').symlink_to(home / 'companion')
            endpoint = temp / 'control.sock'
            with (temp / 'client.log').open('w+') as log:
                before = {int(l.split()[0]) for l in leftovers(home)}
                process = start(client, resources, temp, endpoint, log, offscreen, style=style)
                try:
                    wait_for(endpoint, 'screen', lambda v: v['screen'] == 'title', timeout=60, what='the title')
                    command(endpoint, 'singleplayer')
                    time.sleep(4)
                    command(endpoint, f'screenshot {shots / f"create-world-{style}.png"}')
                    open_world(endpoint, 'Pages')
                    command(endpoint, 'options companion')
                    time.sleep(3)
                    command(endpoint, f'screenshot {shots / f"companion-page-{style}.png"}')
                    time.sleep(5)  # the voice rows appear once the setup check has run
                    command(endpoint, 'options companion 11')
                    time.sleep(1)
                    command(endpoint, f'screenshot {shots / f"companion-page-{style}-companions.png"}')
                    command(endpoint, 'quit')
                    assert process.wait(timeout=60) == 0
                    time.sleep(2)
                    left = leftovers(home, before) + leftovers(temp, before)
                    assert not left, left
                    print(style, 'pages captured')
                except Exception:
                    log.seek(0)
                    print('\n'.join(log.read().splitlines()[-40:]))
                    raise
                finally:
                    stop(process)


def scenario_fresh(client, resources, offscreen):
    """A home that never turned the companion on: nothing appears, nothing starts, even with a
    companion spawned by the dev command (the rules alone answer it)."""
    with tempfile.TemporaryDirectory(prefix='horizon-fresh-') as directory:
        home = Path(directory)
        endpoint = home / 'control.sock'
        with (home / 'client.log').open('w+') as log:
            process = start(client, resources, home, endpoint, log, offscreen)
            try:
                open_world(endpoint, 'Fresh')
                # The Companion section looks at the PC and the plan: still nothing on disk.
                command(endpoint, 'options companion')
                time.sleep(3)
                command(endpoint, f'screenshot {home / "fresh-companion-page.png"}')
                command(endpoint, 'key esc tap')
                command(endpoint, 'key esc tap')
                state = ai(endpoint)
                assert not state['running'] and not state['brain'], state
                command(endpoint, 'companion spawn Kev')
                time.sleep(3)
                lines, _ = hear(endpoint, 'Kev, follow me')
                assert lines and companion(endpoint)['skill'] == 'follow', lines
                lines, _ = hear(endpoint, 'Kev, what is your name?')
                state = ai(endpoint)
                assert not state['running'], state
                kids = children(process.pid)
                assert not kids, kids
                assert not (home / 'companion').exists(), list((home / 'companion').rglob('*'))
                command(endpoint, 'quit')
                process.wait(timeout=30)
            except Exception:
                log.seek(0)
                print('\n'.join(log.read().splitlines()[-40:]))
                raise
            finally:
                stop(process)
        print('fresh home: no companion folder, no child process; rules answered:', [s['line'] for s in lines])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--client', type=Path, required=True)
    parser.add_argument('--resources', type=Path, default=ROOT / '.local/horizon-client/26.3')
    parser.add_argument('--home', type=Path)
    parser.add_argument('--offscreen', action='store_true')
    parser.add_argument('--scenario', default='mind',
                        choices=['mind', 'demo', 'session', 'cost', 'fresh', 'suitup', 'salty', 'latency',
                                 'pages'])
    parser.add_argument('--language', default=None, help='the game language (lang: in options.txt)')
    parser.add_argument('--wav', type=Path, help='write every spoken line here as WAV')
    parser.add_argument('--ptt', type=Path, help='a WAV to send through push-to-talk (demo)')
    parser.add_argument('--shots', type=Path)
    parser.add_argument('--minutes', type=float, default=20)
    parser.add_argument('--keep-world', action='store_true')
    args = parser.parse_args()
    client, resources = args.client.resolve(), args.resources.resolve()
    if args.scenario == 'fresh':
        scenario_fresh(client, resources, args.offscreen)
        return
    home = args.home.resolve()
    if args.scenario == 'pages':
        scenario_pages(client, resources, home, args.shots, args.offscreen)
        return
    if args.language:
        options = home / 'options.txt'
        lines = options.read_text().splitlines() if options.exists() else []
        lines = [l for l in lines if not l.startswith('lang:')] + [f'lang:{args.language}']
        options.write_text('\n'.join(lines) + '\n')
    world = {'mind': 'CompanionMind', 'demo': 'CompanionDemo', 'session': 'CompanionSession',
             'cost': 'CompanionCost', 'suitup': 'CompanionSuitUp', 'salty': 'CompanionSalty',
             'latency': 'CompanionLatency'}[args.scenario]
    if not args.keep_world:
        shutil.rmtree(home / 'saves' / world, ignore_errors=True)
    for folder in (args.wav, args.shots):
        if folder:
            folder.mkdir(parents=True, exist_ok=True)
    endpoint = home / 'control.sock'
    if endpoint.exists():
        endpoint.unlink()
    log_path = home / f'client-{args.scenario}.log'
    with log_path.open('w+') as log:
        before = {int(l.split()[0]) for l in leftovers(home)}
        process = start(client, resources, home, endpoint, log, args.offscreen, args.wav)
        try:
            started = time.monotonic()
            if args.scenario == 'mind':
                result = scenario_mind(endpoint, args.shots, args.language)
            elif args.scenario == 'demo':
                result = scenario_demo(endpoint, args.shots, args.language, args.ptt)
            elif args.scenario == 'session':
                result = scenario_session(endpoint, args.minutes)
            elif args.scenario == 'suitup':
                result = scenario_suitup(endpoint, args.shots, home)
            elif args.scenario == 'salty':
                result = scenario_salty(endpoint, args.shots)
            elif args.scenario == 'latency':
                result = scenario_latency(endpoint)
            else:
                result = scenario_cost(endpoint)
            kids = children(process.pid)
            print('running beside the game:', [k[1].split()[0].rsplit('/', 1)[-1] for k in kids])
            command(endpoint, 'quit')
            code = process.wait(timeout=60)
            assert code == 0, f'client exited with {code}'
            time.sleep(2)
            left = leftovers(home, before)
            assert not left, left
            print(f'quit cleanly after {time.monotonic() - started:.0f} s; nothing left running')
            if args.scenario == 'suitup':
                suitup_check(result)
            print(json.dumps(result, indent=1)[:6000])
        except Exception:
            log.seek(0)
            print('\n'.join(log.read().splitlines()[-60:]))
            raise
        finally:
            stop(process)
    panics = [l for l in log_path.read_text().splitlines() if 'panicked' in l]
    assert not panics, panics


if __name__ == '__main__':
    main()
