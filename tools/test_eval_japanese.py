"""Offline fixture tests for Japanese benchmark scoring and query construction."""

import contextlib
import hashlib
import io
import json
import sys
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from eval_japanese import broad_metrics, canonical_page, make_query, pinpoint_summary, rank_of


class JapaneseBenchmarkTests(unittest.TestCase):
    def test_additive_qiita_requires_a_baseline_without_qiita(self):
        import eval_japanese

        for extra in ([], ["--engines", "qiita"], ["--engines", "wikipedia,qiita"]):
            with self.subTest(extra=extra), \
                    patch.object(sys, "argv", ["eval_japanese.py", "--dry-run", *extra]), \
                    patch.object(eval_japanese, "server_snapshot") as snapshot, \
                    patch.object(eval_japanese, "search") as search, \
                    contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    eval_japanese.main()
                self.assertEqual(error.exception.code, 2)
                snapshot.assert_not_called()
                search.assert_not_called()

    def test_dry_run_counts_the_matrix_without_network_or_output_files(self):
        import eval_japanese

        with tempfile.TemporaryDirectory() as directory:
            diagnostics = Path(directory) / "diagnostics.json"
            diagnostics.write_text(json.dumps({"cases": [
                {"id": "one", "exact_error": "エラー", "keyword_terms": ["エラー"]}
            ]}), encoding="utf-8")
            broad = Path(directory) / "broad.json"
            broad.write_text(json.dumps({"cases": [
                {"id": "two", "query": "recent changes"}
            ]}), encoding="utf-8")
            output = Path(directory) / "output"
            argv = ["eval_japanese.py", "--dry-run", "--output", str(output),
                    "--engines", "wikipedia", "--variants", "raw,quoted",
                    "--diagnostics-json", str(diagnostics), "--broad-fresh-json", str(broad)]
            stdout = io.StringIO()
            with patch.object(sys, "argv", argv), \
                    patch.object(eval_japanese, "server_snapshot") as snapshot, \
                    patch.object(eval_japanese, "search") as search, \
                    contextlib.redirect_stdout(stdout):
                eval_japanese.main()
            self.assertFalse(output.exists())
            snapshot.assert_not_called()
            search.assert_not_called()
            plan = json.loads(stdout.getvalue())
            self.assertEqual(plan["search_requests"], 24)
            self.assertEqual(plan["requests_by_group"], {"pinpoint": 16, "broad_fresh": 8})
            self.assertEqual(plan["qiita_selected_search_requests"], 12)
            self.assertEqual(plan["engine_conditions"], [
                {"qiita": False, "requested_engines": ["wikipedia"], "search_requests": 12},
                {"qiita": True, "requested_engines": ["wikipedia", "qiita"], "search_requests": 12},
            ])
            self.assertIsNone(plan["upstream_request_count"])
            for group, path in (("pinpoint", diagnostics), ("broad_fresh", broad)):
                self.assertEqual(plan["corpora"][group]["sha256"], hashlib.sha256(path.read_bytes()).hexdigest())

    def test_invalid_matrix_values_are_rejected_before_network_access(self):
        import eval_japanese

        for option, value in (("--qiita", "of,on"), ("--variants", "raw,raw"),
                              ("--groups", "pinpiont"), ("--languages", "ja,")):
            with self.subTest(option=option, value=value), \
                    patch.object(sys, "argv", ["eval_japanese.py", "--dry-run", "--engines", "wikipedia", option, value]), \
                    patch.object(eval_japanese, "server_snapshot") as snapshot, \
                    contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    eval_japanese.main()
                snapshot.assert_not_called()

    def test_existing_evidence_is_not_overwritten(self):
        import eval_japanese

        with tempfile.TemporaryDirectory() as directory:
            saved = Path(directory) / "metadata.json"
            saved.write_text("original evidence", encoding="utf-8")
            argv = ["eval_japanese.py", "--output", directory, "--qiita", "off"]
            with patch.object(sys, "argv", argv), \
                    patch.object(eval_japanese, "server_snapshot") as snapshot, \
                    contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    eval_japanese.main()
                snapshot.assert_not_called()
            self.assertEqual(saved.read_text(encoding="utf-8"), "original evidence")

    def test_corpus_has_30_pinned_official_japanese_diagnostics(self):
        corpus = json.loads((Path(__file__).resolve().parents[1] / "bench/japanese-search/diagnostics.json").read_text(encoding="utf-8"))
        self.assertEqual(len(corpus["cases"]), 30)
        for case in corpus["cases"]:
            self.assertEqual(case["typescript_version"], "5.9.3")
            self.assertIn("microsoft/TypeScript/blob/v5.9.3/lib/ja/diagnosticMessages.generated.json", case["source_url"])
            self.assertTrue(case["expected_reference_urls"])
            self.assertTrue(case["exact_error"])
            instantiated = case["diagnostic_message_template"]
            for index, value in enumerate(case["template_arguments"]):
                instantiated = instantiated.replace("{" + str(index) + "}", value)
            self.assertEqual(instantiated, case["exact_error"])

    def test_transforms_are_distinct_and_recordable(self):
        case = {"exact_error": "名前 'x' が見つかりません。", "keyword_terms": ["名前", "x"]}
        self.assertEqual(make_query(case, "raw"), "名前 'x' が見つかりません。")
        self.assertEqual(make_query(case, "quoted"), '"名前 \'x\' が見つかりません。"')
        self.assertEqual(make_query(case, "keywords"), "名前 x")

    def test_qiita_and_language_are_independent_request_conditions(self):
        from eval_japanese import build_search_url
        baseline = parse_qs(urlsplit(build_search_url("http://localhost", "query", "default", False, None, False, baseline_engines=["duckduckgo", "bing"])).query)
        language = parse_qs(urlsplit(build_search_url("http://localhost", "query", "ja", False, None, False, baseline_engines=["duckduckgo", "bing"])).query)
        qiita = parse_qs(urlsplit(build_search_url("http://localhost", "query", "default", True, None, False, baseline_engines=["duckduckgo", "bing"])).query)
        self.assertNotIn("language", baseline)
        self.assertEqual(language["language"], ["ja"])
        self.assertEqual(baseline["engines"], ["duckduckgo,bing"])
        self.assertEqual(language["engines"], ["duckduckgo,bing"])
        self.assertEqual(qiita["engines"], ["duckduckgo,bing,qiita"])
        self.assertNotIn("language", qiita)

    def test_upstream_camel_case_publication_date_is_preserved(self):
        from eval_japanese import search
        from io import BytesIO
        response = BytesIO(json.dumps({"results": [{"url": "https://example.org/", "publishedDate": "2026-11-02"}]}).encode())
        response.status = 200
        with patch("urllib.request.urlopen", return_value=response):
            row = search("http://localhost", "test", "ja", False, None, False, 1)
        self.assertEqual(row["results"][0]["published_date"], "2026-11-02")

    def test_canonical_rank_requires_same_page(self):
        self.assertEqual(rank_of(["https://www.example.org/a/#frag"], ["http://example.org/a"]), 1)
        self.assertIsNone(rank_of(["https://example.org/a-extra"], ["https://example.org/a"]))
        self.assertEqual(rank_of(["https://example.org/%E6%9D%B1%E4%BA%AC"], ["https://example.org/東京"]), 1)
        self.assertEqual(canonical_page("https://example.org/a"), ("example.org", "/a", ""))

    def test_pinpoint_summary_counts_failures_and_latency(self):
        result = pinpoint_summary([
            {"rank": 1, "elapsed_ms": 50}, {"rank": 5, "elapsed_ms": 150},
            {"rank": None, "elapsed_ms": 250, "error": "timeout"},
        ])
        self.assertEqual(result["reference_hit@1"], 1)
        self.assertEqual(result["reference_hit@5"], 2)
        self.assertEqual(result["failures"], 1)
        self.assertEqual(result["request_errors"], 1)
        self.assertEqual(result["p50_ms"], 150)

    def test_broad_metrics_measure_diversity_duplicates_and_date_coverage(self):
        row = {
            "target_source_domains": ["a.example", "b.example"],
            "results": [
                {"url": "https://a.example/1", "published_date": "2026-10-30T12:00:00Z"},
                {"url": "https://a.example/1#fragment", "published_date": None},
                {"url": "https://b.example/2", "published_date": "2020-01-01"},
                {"url": "https://c.example/3", "published_date": "not-a-date"},
            ],
        }
        result = broad_metrics(row, 30)
        self.assertEqual(result["unique_domains"], 3)
        self.assertEqual(result["duplicate_pages"], 1)
        self.assertEqual(result["publication_date_count"], 2)
        self.assertEqual(result["publication_date_missing_count"], 1)
        self.assertEqual(result["publication_date_invalid_count"], 1)
        self.assertEqual(result["target_source_domain_coverage"], 1.0)
        self.assertGreaterEqual(result["within_time_window_count"], 1)

    def test_main_scores_target_domains_attached_from_the_case(self):
        """Observation rows from main() must be scored against the case's target domains."""
        import sys
        import tempfile
        import eval_japanese

        corpus = {"cases": [{"id": "fresh-x", "query": "q", "time_range": "month", "window_days": 30,
                             "target_source_domains": ["a.example", "b.example"]}]}
        response = {"http_status": 200, "results": [{"url": "https://www.a.example/1"}, {"url": "https://c.example/2"}],
                    "elapsed_ms": 1.0, "error": None}
        with tempfile.TemporaryDirectory() as directory:
            broad = Path(directory) / "broad.json"
            broad.write_text(json.dumps(corpus), encoding="utf-8")
            output = Path(directory) / "out"
            argv = ["eval_japanese.py", "--output", str(output), "--groups", "broad_fresh", "--languages", "default",
                    "--qiita", "off", "--repeats", "2", "--delay", "0", "--broad-fresh-json", str(broad)]
            with patch.object(sys, "argv", argv), \
                    patch.object(eval_japanese, "search", side_effect=lambda *a, **k: dict(response)), \
                    patch.object(eval_japanese, "server_snapshot", return_value={}), \
                    patch("builtins.print"):
                eval_japanese.main()
            rows = [json.loads(line) for line in (output / "observations.jsonl").read_text().splitlines()]
            summary = json.loads((output / "summary.json").read_text())["summaries"]
        self.assertEqual(len(rows), 2)
        for row in rows:
            self.assertEqual(row["broad_fresh_metrics"]["target_source_domains_seen"], ["a.example"])
            self.assertEqual(row["broad_fresh_metrics"]["target_source_domain_coverage"], 0.5)
        self.assertEqual(summary["broad_fresh/raw/default/qiita=False"]["mean_target_source_domain_coverage"], 0.5)


if __name__ == "__main__":
    unittest.main()
