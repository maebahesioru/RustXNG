#!/usr/bin/env python3
"""Reject vulnerabilities, new warnings and expired known-finding baselines."""
import argparse
import json
from datetime import date
from pathlib import Path


def check(report: dict, baseline: dict, today: date) -> list[str]:
    errors = []
    if report.get('error') or 'vulnerabilities' not in report or 'warnings' not in report:
        return ['audit did not produce a complete report']
    if baseline['findings'] and today >= date.fromisoformat(baseline['expires']):
        errors.append('known-finding baseline expired; review tracked findings')
    for entry in report['vulnerabilities']['list']:
        errors.append('vulnerability: ' + entry['advisory']['id'])
    known = {(item['id'], item['kind'], item['package'], item['version']) for item in baseline['findings']}
    for kind, entries in report['warnings'].items():
        for entry in entries:
            finding = (entry['advisory']['id'], kind, entry['package']['name'], entry['package']['version'])
            print('audit warning:', ' / '.join(finding))
            if finding not in known:
                errors.append('new warning: ' + ' / '.join(finding))
    return errors


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path)
    parser.add_argument('--baseline', type=Path, default=Path('.github/security-baseline.json'))
    args = parser.parse_args()
    report = json.loads(args.report.read_text(encoding='utf-8'))
    baseline = json.loads(args.baseline.read_text(encoding='utf-8'))
    errors = check(report, baseline, date.today())
    if errors:
        raise SystemExit('\n'.join(errors))
    if baseline['findings']:
        print('No vulnerabilities or new warnings; known findings remain tracked until', baseline['expires'])
    else:
        print('No vulnerabilities or warnings.')


if __name__ == '__main__':
    main()
