"""Check that documentation counts agree with the shipped configuration and data.

This does not contact search providers or assert legal permissions.
"""
from __future__ import annotations

import csv
import json
import re
import tomllib
import unittest
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


class DocumentationConsistencyTests(unittest.TestCase):
    def test_provider_inventory_counts(self) -> None:
        settings = tomllib.loads((ROOT / "settings.toml").read_text(encoding="utf-8"))
        with (ROOT / "docs/providers.csv").open(encoding="utf-8", newline="") as stream:
            inventory = list(csv.DictReader(stream))

        entries = settings["engines"]
        self.assertEqual([item["name"] for item in inventory], [item["name"] for item in entries])
        enabled = sum(
            not item.get("disabled", False) and not item.get("inactive", False)
            for item in entries
        )
        unreviewed = sum(item["review_status"] == "unreviewed" for item in inventory)
        text = (ROOT / "docs/PROVIDERS.md").read_text(encoding="utf-8")

        counts = re.search(
            r"全(\d+)エントリ（設定上有効(\d+)、無効(\d+)）", text
        )
        self.assertIsNotNone(counts, "Document the current inventory counts in docs/PROVIDERS.md")
        self.assertEqual(tuple(map(int, counts.groups())), (len(entries), enabled, len(entries) - enabled))
        statuses = re.search(r"(\d+)件中(\d+)件は `unreviewed`", text)
        self.assertIsNotNone(statuses, "Document how many entries remain unreviewed")
        self.assertEqual(tuple(map(int, statuses.groups())), (len(entries), unreviewed))

    def test_comparison_language_counts(self) -> None:
        questions = json.loads((ROOT / "bench/quality-questions-100.json").read_text(encoding="utf-8"))["queries"]
        observed = json.loads((ROOT / "bench/results/profiles-2026-11-04/standard/docs100.json").read_text(encoding="utf-8"))["rows"]
        expected = Counter(question["lang"] for question in questions)
        self.assertEqual(expected, Counter(row["lang"] for row in observed))
        comparison = (ROOT / "docs/COMPARISON-2026-11-04.md").read_text(encoding="utf-8")
        documented = re.search(r"英語(\d+)問・日本語(\d+)問", comparison)
        self.assertIsNotNone(documented)
        self.assertEqual(tuple(map(int, documented.groups())), (expected["en"], expected["ja"]))

    def test_clean_profile_runs_no_engine(self) -> None:
        clean = tomllib.loads((ROOT / "settings.clean.toml").read_text(encoding="utf-8"))
        self.assertTrue(clean.get("use_default_settings") is False)
        self.assertNotIn("engines", clean)
        docs = (ROOT / "docs/CLEAN-PROFILE.md").read_text(encoding="utf-8")
        self.assertIn("外部エンジン 0", docs)
    def test_latest_sealqa_reference_is_not_old_run(self) -> None:
        summary = json.loads(
            (ROOT / "bench/results/profiles-2026-11-05/clean/sealqa/summary.json").read_text(encoding="utf-8")
        )["splits"]["seal_hard"]
        docs = (ROOT / "docs/CLEAN-PROFILE.md").read_text(encoding="utf-8")
        self.assertIn(f'{summary["reference_hit@10"]}/{summary["questions"]}', docs)
        self.assertIn("hit@10 | 85", docs)
        self.assertIn("結果ゼロの質問 | 0", docs)
        self.assertNotIn("結果ゼロが 33 問あります", docs)

    def test_evaluator_does_not_claim_to_detect_index_state(self) -> None:
        source = (ROOT / "tools/eval_sealqa.py").read_text(encoding="utf-8")
        self.assertIn('"index_state": "not determined by this evaluator', source)
        self.assertNotIn("; no local index;", source)


if __name__ == "__main__":
    unittest.main()
