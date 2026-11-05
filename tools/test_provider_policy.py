from __future__ import annotations

import copy
import json
import tomllib
import unittest

from check_provider_policy import ROOT, identity, spacing_for, validate


class ProviderPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.settings = tomllib.loads((ROOT / "settings.toml").read_text(encoding="utf-8"))
        self.policy = json.loads((ROOT / "docs/provider-policy.json").read_text(encoding="utf-8"))
        self.pending = json.loads((ROOT / "docs/provider-review-pending.json").read_text(encoding="utf-8"))["engines"]

    def test_shipped_standard_passes(self) -> None:
        self.assertEqual(validate(self.settings, self.policy, self.pending), [])

    def test_blocked_module_cannot_be_enabled_under_another_name(self) -> None:
        self.settings["engines"].append({"name": "new bing alias", "engine": "bing"})
        self.assertIn("must be disabled", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_shared_module_does_not_approve_a_new_provider(self) -> None:
        self.settings["engines"].append({"name": "new wiki", "engine": "mediawiki"})
        self.assertIn("permission review", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_allowlist_reenablement_cannot_bypass_permission_gate(self) -> None:
        self.settings["search"]["engine_allowlist"] = ["bing"]
        self.assertIn("must be disabled", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_unchanged_pending_snapshot_does_not_authorize_enablement(self) -> None:
        name = next(iter(self.pending))
        engine = next(e for e in self.settings["engines"] if e["name"] == name)
        engine["disabled"] = False
        self.assertIn("permission review", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_generic_module_cannot_inherit_another_sites_permission(self) -> None:
        self.policy["providers"]["mediawiki"] = copy.deepcopy(self.policy["engines"]["wikibooks"])
        self.settings["engines"].append({"name":"another wiki", "engine":"mediawiki"})
        self.assertIn("exact named endpoint", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_named_review_cannot_be_reused_for_another_adapter(self) -> None:
        engine = next(e for e in self.settings["engines"] if e["name"] == "wikibooks")
        engine["engine"] = "json_engine"
        self.assertIn("does not match the configured module", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_adapter_changes_require_review(self) -> None:
        self.policy["providers"]["mwmbl"]["source_sha256"] = "0" * 64
        self.assertIn("adapter changed", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_named_review_cannot_override_blocked_module(self) -> None:
        self.policy["engines"]["new bing alias"] = {"standard_allowed": True, "module": "bing"}
        self.settings["engines"].append({"name": "new bing alias", "engine": "bing"})
        self.assertIn("must be disabled", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_pending_endpoint_cannot_change_without_review(self) -> None:
        name = next(iter(self.pending))
        engine = next(e for e in self.settings["engines"] if e["name"] == name)
        engine["disabled"] = False
        engine["base_url"] = "https://different.example.org"
        self.assertNotEqual(identity(engine), self.pending[name])
        self.assertTrue(validate(self.settings, self.policy, self.pending))

    def test_reviewed_module_does_not_approve_an_unreviewed_endpoint(self) -> None:
        engine = next(e for e in self.settings["engines"] if e["name"] == "mwmbl")
        engine["base_url"] = "https://unreviewed.example.org/api/v2"
        self.assertIn("endpoint review", " ".join(validate(self.settings, self.policy, self.pending)))

    def test_nominatim_spacing_cannot_be_removed_or_disabled(self) -> None:
        for millis in (None, 0, 999):
            settings = copy.deepcopy(self.settings)
            overrides = settings["outgoing"]["host_spacing_overrides"]
            if millis is None:
                del overrides["nominatim.openstreetmap.org"]
            else:
                overrides["nominatim.openstreetmap.org"] = millis
            self.assertIn("nominatim.openstreetmap.org requires", " ".join(validate(settings, self.policy, self.pending)))

    def test_longest_domain_override_wins(self) -> None:
        outgoing = {"host_spacing_ms": 0, "host_spacing_overrides": {"openstreetmap.org": 1100, "nominatim.openstreetmap.org": 0}}
        self.assertEqual(spacing_for(outgoing, "nominatim.openstreetmap.org"), 0)
        self.assertEqual(spacing_for(outgoing, "notopenstreetmap.org"), 0)

    def test_review_metadata_is_required(self) -> None:
        for field in ("terms_url", "review_date", "conditions"):
            policy = copy.deepcopy(self.policy)
            del policy["providers"]["openstreetmap"][field]
            self.assertTrue(validate(self.settings, policy, self.pending), field)


if __name__ == "__main__":
    unittest.main()
