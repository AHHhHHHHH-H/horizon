#!/usr/bin/env python3
"""Compare item tooltip display visibility with the pinned vanilla client."""
import json
try:
    from . import lab, oracle_harness as harness
except ImportError:
    import lab, oracle_harness as harness


def cases(build):
    values = [
        (False, False, False),
        (False, False, True),
        (False, True, True),
        (True, False, False),
        (True, True, False),
        (True, True, True),
    ]
    path = build / 'cases.txt'
    # Lowercase: Rust's bool parser takes only true/false (Java's parseBoolean ignores case).
    names = [
        'Plain name',
        {'text': 'Coverage sword', 'bold': True, 'color': 'gold', 'italic': False},
        {'text': '', 'bold': True, 'color': 'red', 'extra': [
            {'text': 'Queue '}, {'text': '(Right Click)', 'bold': False, 'color': 'gray', 'italic': False}]},
        {'text': 'RGB', 'color': '#12ab34', 'underlined': True, 'strikethrough': True},
        {'text': 'A', 'italic': False, 'extra': [{'text': 'B', 'italic': True}, {'text': 'C', 'bold': True}]},
        [{'text': 'List ', 'bold': True}, {'text': 'tail', 'bold': False}],
        {'text': '日本語 🎮', 'obfuscated': False},
    ]
    path.write_text('\n'.join('\t'.join(str(v).lower() for v in row) for row in values) + '\n'
                    + ''.join('name\t' + json.dumps(name, ensure_ascii=False) + '\n' for name in names))
    return path, len(values) + len(names)


if __name__ == '__main__':
    harness.differential(
        'tooltip-display-parity', 'VanillaTooltipDisplayOracle', 'tooltip_display_fixture', cases,
        'Tooltip visibility and styled custom-name character colours, bold, italic and decoration inheritance',
        sources=[
            lab.ROOT / 'crates/horizon-client/src/game/stations.rs',
            lab.ROOT / 'crates/horizon-client/src/item_tooltip.rs',
            lab.ROOT / 'crates/horizon-client/src/item_data.rs',
            lab.ROOT / 'crates/horizon-client/src/game/titles.rs',
            lab.ROOT / 'crates/horizon-client/src/game/recipe_book.rs',
        ],
        expected_lines=13)
