#!/usr/bin/env python3
"""Summarize retained coverage and frozen payload observations without requests."""
import gzip
import json
import statistics
from collections import Counter, defaultdict
from pathlib import Path
from reference_urls import first_hit_rank

ROOT = Path(__file__).resolve().parents[1]
RESULTS = ROOT / 'bench/results/issue-resolution-2026-11-05'


def percentile(values, fraction):
    values = sorted(values)
    return round(values[min(len(values)-1, int((len(values)-1)*fraction+.5))], 3) if values else 0


def main():
    coverage = {}
    for path in sorted((RESULTS / 'coverage').glob('*.json.gz')):
        report = json.loads(gzip.decompress(path.read_bytes()))
        rows = report['rows']
        errors = Counter(str(error[1]) for row in rows for error in row.get('payload', {}).get('unresponsive_engines', []))
        coverage[path.name.removesuffix('.json.gz')] = {
            **{key: report[key] for key in ['questions','hit@1','hit@5','mrr@10','median_ms','errors']},
            'hit@10':sum(row['rank'] is not None for row in rows),
            'p95_ms':percentile([row['millis'] for row in rows], .95),
            'zero_results':sum(not row['urls'] for row in rows),
            'provider_failure_reports_including_suspensions':dict(errors),
        }
    packed = json.loads(gzip.decompress((RESULTS/'agent-payloads.json.gz').read_bytes()))
    groups = defaultdict(list)
    for row in packed['rows']:
        groups[row['condition']].append(row)
    payloads = {}
    for condition, rows in groups.items():
        ranks = [first_hit_rank(row['urls'][:10], row['gold']) for row in rows]
        times = [sample for row in rows for sample in row.get('packing_ms', [])]
        payloads[condition] = {
            'questions':len(rows),'hit@1':sum(rank==1 for rank in ranks),
            'hit@5':sum(rank is not None and rank<=5 for rank in ranks),
            'hit@10':sum(rank is not None for rank in ranks),
            'mrr@10':round(sum(1/rank for rank in ranks if rank)/len(rows),3),
            'median_bytes':statistics.median(row['bytes'] for row in rows),
            'median_tokens':{enc:statistics.median(row['tokens'][enc] for row in rows) for enc in ['cl100k_base','o200k_base']},
            'packing_p95_ms':percentile(times,.95) if times else None,
            'source_urls':sum(len(row['urls']) for row in rows),
            'leading_80_chars_preserved':sum(row.get('leading_80_chars_preserved',0) for row in rows) if times else None,
        }
    result={'coverage':coverage,'payloads':payloads,'tokenizer':packed['tokenizer_implementation'],'count_scope':packed['count_scope'],'limitations':packed['protocol']}
    (RESULTS/'summary.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps(result,ensure_ascii=False,indent=2))


if __name__ == '__main__':
    main()
