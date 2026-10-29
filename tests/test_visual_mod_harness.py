"""The visual fixture launches isolated games and retains join failures for diagnosis."""
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

from tools.visual_mod_harness import Game


class VisualFixtureTests(unittest.TestCase):
    def game(self, root):
        output = root / 'output'
        output.mkdir()
        args = SimpleNamespace(ctl=root / 'ctl', client=root / 'client', resources=root / 'resources', output=output)
        return Game(args, root, 'guest')

    def test_title_launch_does_not_wait_for_a_world_before_connecting(self):
        with tempfile.TemporaryDirectory() as scratch:
            game = self.game(Path(scratch))
            with patch.object(game, 'ctl', return_value={'launch': {'pid': 42}}) as ctl:
                game.launch(mod=False, world=False)
            arguments = ctl.call_args.args
            self.assertNotIn('--world', arguments)
            self.assertNotIn('--wait', arguments)
            self.assertEqual(game.pid, 42)

    def test_commands_and_inherited_client_cwd_stay_in_scratch_home(self):
        with tempfile.TemporaryDirectory() as scratch:
            game = self.game(Path(scratch))
            with patch('tools.visual_mod_harness.subprocess.run', return_value=SimpleNamespace(
                    returncode=0, stdout='{"ok":true}\n', stderr='')) as run:
                game.ctl('status')
            self.assertEqual(run.call_args.kwargs['cwd'], game.home)
            self.assertEqual(run.call_args.kwargs['env']['HORIZON_HOME'], str(game.home))

    def test_failed_commands_keep_complete_diagnostics_and_scratch_logs(self):
        with tempfile.TemporaryDirectory() as scratch:
            game = self.game(Path(scratch))
            (game.home / 'logs').mkdir()
            (game.home / 'logs/game.log').write_text('join rejected\n')
            with patch('tools.visual_mod_harness.subprocess.run', return_value=SimpleNamespace(
                    returncode=1, stdout='full state', stderr='join failed')):
                with self.assertRaises(RuntimeError):
                    game.ctl('connect', '127.0.0.1:1', 'VisualGuest')
            report = json.loads((game.args.output / 'guest-failure.json').read_text())
            self.assertEqual(report['stdout'], 'full state')
            self.assertEqual((game.args.output / 'guest-logs/game.log').read_text(), 'join rejected\n')


if __name__ == '__main__':
    unittest.main()
