import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("score", Path(__file__).with_name("score-quality-corpus.py"))
scorer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scorer)


class QualityScoreTests(unittest.TestCase):
    def test_numbers_negation_and_identifiers_survive(self):
        self.assertEqual(scorer.tokens("不要支付 ￥１２３.４５，not 12345！", "mixed"),
                         ["不", "要", "支", "付", "¥123.45", "not", "12345"])
        self.assertNotEqual(scorer.tokens("3.5", "en"), scorer.tokens("35", "en"))
        self.assertEqual(scorer.tokens("Don’t email A.B@example.com.", "en"),
                         ["don't", "email", "a.b@example.com"])
        self.assertNotEqual(scorer.tokens("-5%", "en"), scorer.tokens("5", "en"))
        self.assertEqual(scorer.tokens("https://a.b/x 12:30", "en"), ["https://a.b/x", "12:30"])

    def test_mixed_word_boundaries_and_case(self):
        self.assertEqual(scorer.tokens("我要play Basketball，明天。", "mixed"),
                         ["我", "要", "play", "basketball", "明", "天"])
        self.assertEqual(scorer.tokens("Hello, WORLD!", "en"), ["hello", "world"])
        self.assertNotEqual(scorer.tokens("开发", "zh"), scorer.tokens("開發", "zh"))

    def test_edit_operations(self):
        self.assertEqual(scorer.edits(["a", "b", "c"], ["a", "x", "c", "d"]),
                         dict(errors=2, substitutions=1, deletions=0, insertions=1, reference_tokens=3))
        self.assertEqual(scorer.edits(["not", "now"], [])['deletions'], 2)
        self.assertEqual(scorer.edits([], ["x"])['insertions'], 1)
        self.assertEqual(scorer.edits([], [])['errors'], 0)

    def fixture(self):
        manifest = json.dumps(dict(purpose="public_development_diagnostic", limitations=["fixture"], clips=[
            dict(id="one", language="en", reference="do not pay 3.5", samples=16000, speaker=None),
            dict(id="two", language="en", reference="hello", samples=16000, speaker=None)])).encode()
        rows = [dict(type="header", manifest_sha256=scorer.sha(manifest), clips=2, models=["model"]),
                dict(type="result", id="one", model_id="model", status="failed", error="NoSpeech", text="", wall_ms=10),
                dict(type="result", id="two", model_id="model", status="completed", text="hello", wall_ms=20, accepted_events=1),
                dict(type="complete")]
        return manifest, rows

    def test_failures_count_as_deletions_and_not_latency_successes(self):
        manifest, rows = self.fixture()
        report = scorer.score(manifest, '\n'.join(map(json.dumps, rows)).encode())
        group = report['summaries'][0]
        self.assertEqual((group['deletions'], group['reference_tokens']), (4, 5))
        self.assertEqual(group['error_rate'], .8)
        self.assertEqual(group['completed_wall_ms']['n'], 1)
        self.assertFalse(report['release_gate_passed'])

    def test_incomplete_duplicate_and_changed_input_rejected(self):
        manifest, rows = self.fixture()
        variants = [rows[:-1], [rows[0], *rows[2:]], [rows[0], rows[1], *rows[1:]]]
        for variant in variants:
            with self.assertRaises(ValueError):
                scorer.score(manifest, '\n'.join(map(json.dumps, variant)).encode())
        with self.assertRaises(ValueError):
            scorer.score(manifest + b' ', '\n'.join(map(json.dumps, rows)).encode())

    def test_nearest_rank_not_mean(self):
        result = scorer.quantiles(list(range(1, 101)))
        self.assertEqual((result['p50'], result['p95']), (50, 95))


if __name__ == "__main__":
    unittest.main()
