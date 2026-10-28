#!/usr/bin/env python3
"""Real-client API fixtures, isolated from live worlds and settings.

Requires client/ctl built with mods and the Effect Lab .hmod from the sandbox
suite (or cargo horizon-mod build). Tests in-frame, server-thread and TCP guest
clients; captures screenshots, CPU/GPU summaries and cleanup diagnostics.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

try:
    from . import lab
except ImportError:
    import lab


class Game:
    def __init__(self, args, root, name):
        self.args = args
        self.home = root / name
        self.home.mkdir()
        self.socket = self.home / 'ctl.sock'
        self.pid = None
        base = {k: v for k, v in os.environ.items() if not k.startswith('HORIZON_VOICE_')}
        self.env = lab.game_env(
            base,
            HORIZON_HOME=self.home,
            HORIZON_LAUNCHER_HOME=self.home / 'launcher',
            XDG_DATA_HOME=self.home / 'xdg-data',
            XDG_CONFIG_HOME=self.home / 'xdg-config',
            XDG_CACHE_HOME=self.home / 'xdg-cache',
            HORIZON_LAUNCHER_TRASH=self.home / 'trash',
            HORIZON_LAUNCHER_CLIPBOARD=self.home / 'clipboard',
            HORIZON_LAUNCHER_DESKTOP=self.home / 'desktop',
        )

    def ctl(self, *words, timeout=300):
        command = [str(self.args.ctl), '--socket', str(self.socket), '--timeout', str(timeout), *map(str, words)]
        result = subprocess.run(command, cwd=self.home, env=self.env, capture_output=True, text=True, timeout=timeout + 30)
        if result.returncode:
            (self.args.output / f'{self.home.name}-failure.json').write_text(json.dumps({
                'command': list(map(str, words)), 'stdout': result.stdout, 'stderr': result.stderr,
            }, indent=2) + '\n')
            logs = self.home / 'logs'
            if logs.is_dir():
                shutil.copytree(logs, self.args.output / f'{self.home.name}-logs', dirs_exist_ok=True)
            raise RuntimeError(f'{words}: {result.stderr[-2000:]} {result.stdout[-2000:]}')
        return json.loads(result.stdout.strip().splitlines()[-1])

    def launch(self, *extra, mod=True, world=True):
        args = ['launch', '--game', self.args.client, '--resources', self.args.resources,
                '--width', '480', '--height', '320', '--render-distance', '2', '--offscreen']
        if world:
            args.extend(['--world', 'API fixture', '--creative', '--test-terrain', '--seed', '1', '--wait'])
        if mod:
            self.mod = self.home / 'effect_lab.hmod'
            shutil.copy2(self.args.hmod, self.mod)
            args.extend(['--dev-mod', self.mod])
        state = self.ctl(*args, *extra)
        self.pid = state.get('launch', {}).get('pid')

    def close(self):
        try:
            self.ctl('quit', timeout=30)
        except Exception:
            if self.pid:
                try:
                    os.kill(self.pid, 15)
                except ProcessLookupError:
                    pass

    def visuals(self):
        return self.ctl('status')['mod_visuals']

    def exercise(self, out):
        self.ctl('chat', '/effect_lab off')
        self.ctl('wait', '40')
        assert self.visuals()['colored_lights'] == 0
        before = self.ctl('bench', '1')
        self.ctl('chat', '/effect_lab on')
        self.ctl('wait', '12')
        active = self.visuals()
        assert active['particles'] > 0, active
        assert active['colored_lights'] == 1, active
        assert active['emissive_mappings'] > 0, active
        assert active['post_effects'] == 1, active
        assert active['post_target_bytes'] > 0, active
        self.ctl('screenshot', out / f'{self.home.name}-on.png')
        during = self.ctl('bench', '1')
        self.ctl('chat', '/effect_lab off')
        self.ctl('wait', '40')
        cleared = self.visuals()
        assert cleared['particles'] == 0 and cleared['colored_lights'] == 0, cleared
        assert cleared['post_effects'] == 0 and cleared['post_target_bytes'] == 0, cleared
        assert cleared['emissive_mappings'] == 0, cleared
        self.ctl('screenshot', out / f'{self.home.name}-off.png')
        # A failed hot reload must also remove already active owned effects.
        self.ctl('chat', '/effect_lab on')
        self.ctl('wait', '8')
        self.mod.write_bytes(b'invalid package for cleanup fixture')
        self.ctl('wait', '60')
        failed = self.visuals()
        assert failed['particles'] == 0 and failed['colored_lights'] == 0, failed
        assert failed['post_effects'] == 0 and failed['post_target_bytes'] == 0, failed
        assert failed['emissive_mappings'] == 0, failed
        return {'mode': self.home.name, 'active': active, 'cleared': cleared,
                'failed_reload': failed, 'off_timing': before, 'on_timing': during}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ['client', 'ctl', 'resources', 'hmod', 'output']:
        parser.add_argument('--' + flag, type=Path, required=True)
    parser.add_argument('--mode', choices=['in-frame', 'server-thread', 'multiplayer', 'all'], default='all')
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    for flag in ['client', 'ctl', 'resources', 'hmod']:
        setattr(args, flag, getattr(args, flag).resolve())
    temp_parent = '/tmp/opencode' if Path('/tmp/opencode').is_dir() else None
    with tempfile.TemporaryDirectory(prefix='visual-mod-fixture-', dir=temp_parent) as scratch:
        root = Path(scratch)
        results = []
        modes = ['in-frame', 'server-thread', 'multiplayer'] if args.mode == 'all' else [args.mode]
        for mode in modes:
            host = None
            game = Game(args, root, mode)
            try:
                if mode == 'multiplayer':
                    host = Game(args, root, 'host')
                    host.launch(mod=False)
                    opened = host.ctl('lan', 'open', '0', 'commands')
                    game.launch(world=False)
                    game.ctl('connect', f'127.0.0.1:{opened["port"]}', 'VisualGuest')
                    game.ctl('wait-for', 'loaded')
                else:
                    game.launch(*(['--in-frame'] if mode == 'in-frame' else []))
                result = game.exercise(args.output)
                results.append(result)
                (args.output / 'report.json').write_text(json.dumps(results, indent=2) + '\n')
                print(json.dumps({'mode': mode, 'passed': True}), flush=True)
            finally:
                game.close()
                if host:
                    host.close()


if __name__ == '__main__':
    main()
