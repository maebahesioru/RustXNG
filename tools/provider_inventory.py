#!/usr/bin/env python3
"""Inventory every shipped engine entry without contacting any provider.

URL literals are code-review candidates, not verified request endpoints.
Unknown access methods, defaults and permissions stay explicitly unknown.
"""
import argparse
import csv
import io
import json
import re
import tomllib
from pathlib import Path
from provider_policy import adapter_source, review_for

ROOT = Path(__file__).resolve().parent.parent


def inventory() -> str:
    settings = tomllib.loads((ROOT / 'settings.toml').read_text(encoding="utf-8"))
    policy = json.loads((ROOT / 'docs/provider-policy.json').read_text(encoding="utf-8"))
    output = io.StringIO(newline='')
    writer = csv.writer(output, lineterminator='\n')
    writer.writerow(['name', 'module', 'disabled_in_config', 'inactive_in_config', 'enabled_in_config', 'source', 'configured_endpoint',
                     'source_url_literals_unverified', 'reviewed_request_url', 'access_method',
                     'terms_url', 'review_status', 'review_date', 'decision', 'standard_allowed', 'conditions'])
    for engine in settings['engines']:
        module = engine.get('engine', engine['name'])
        source = adapter_source(ROOT, module)
        text = source.read_text(encoding="utf-8") if source.exists() else ''
        production = text.split('#[cfg(test)]', 1)[0]
        disabled = engine.get('disabled', False)
        inactive = engine.get('inactive', False)
        entry = review_for(engine, policy)
        writer.writerow([engine['name'], module, str(disabled).lower(), str(inactive).lower(), str(not disabled and not inactive).lower(),
                         source.relative_to(ROOT).as_posix() if source.exists() else 'src/engines/mod.rs (locate registration)',
                         engine.get('base_url') or engine.get('search_url') or '',
                         ' | '.join(sorted(set(re.findall(r'https?://[^\s"<>]+', production)))),
                         entry.get('request_url', ''), entry.get('access_method', 'unreviewed'),
                         entry.get('terms_url', ''), entry.get('status', 'unreviewed'),
                         entry.get('review_date', '') if entry else '', entry.get('decision', 'provider_review_needed'),
                         str(entry['standard_allowed']).lower() if 'standard_allowed' in entry else 'unreviewed',
                         ' | '.join(entry.get('conditions', []))])
    return output.getvalue()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    path = ROOT / 'docs/providers.csv'
    text = inventory()
    if args.check:
        if not path.exists() or path.read_text(encoding="utf-8") != text:
            raise SystemExit('Provider inventory is stale; run python3 tools/provider_inventory.py')
        print('Provider inventory matches every settings entry.')
    else:
        path.write_text(text, encoding="utf-8", newline="\n")
        print(f'Wrote {len(tomllib.loads((ROOT / "settings.toml").read_text(encoding="utf-8"))["engines"])} entries.')


if __name__ == '__main__':
    main()
