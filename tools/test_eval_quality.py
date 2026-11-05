import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import eval_quality


class FailureLatencyTest(unittest.TestCase):
    def test_timeouts_and_instant_failures_remain_in_latency_and_quality(self):
        with tempfile.TemporaryDirectory() as directory:
            questions = Path(directory) / "questions.json"
            report_path = Path(directory) / "report.json"
            questions.write_text(json.dumps({"queries": [
                {"id": str(i), "query": "test", "gold": ["https://example.com/answer"]}
                for i in range(3)
            ]}), encoding="utf-8")
            response = io.BytesIO(json.dumps({"results": [
                {"url": "https://example.com/answer"}
            ]}).encode())
            argv = [
                "eval_quality.py", "--questions", str(questions),
                "--json", str(report_path), "--delay", "0",
            ]
            with (
                patch("sys.argv", argv),
                patch("eval_quality.urllib.request.urlopen", side_effect=[
                    TimeoutError("request timed out"), response, OSError("connection refused"),
                ]),
                patch("eval_quality.time.perf_counter", side_effect=[
                    100.0, 102.0, 200.0, 200.1, 300.0, 300.0,
                ]),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(eval_quality.main(), 1)
            report = json.loads(report_path.read_text(encoding="utf-8"))

        self.assertEqual(report["questions"], 3)
        self.assertEqual(report["errors"], 2)
        self.assertEqual(report["hit@1"], 1)
        self.assertEqual(report["hit@5"], 1)
        self.assertEqual(report["mrr@10"], 0.333)
        self.assertEqual(report["median_ms"], 100.0)
        self.assertEqual([row["rank"] for row in report["rows"]], [None, 1, None])
        self.assertEqual(report["rows"][0]["millis"], 2000.0)
        self.assertEqual(report["rows"][2]["millis"], 0.0)
        self.assertIn("TimeoutError", report["rows"][0]["error"])
        self.assertIn("OSError", report["rows"][2]["error"])


if __name__ == "__main__":
    unittest.main()
