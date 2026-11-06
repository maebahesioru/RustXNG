#!/usr/bin/env python3
"""Check Standard defaults; a documented pending review is not permission approval."""
from __future__ import annotations

import hashlib
import json
import tomllib
from datetime import date
from pathlib import Path
from provider_policy import GENERIC_MODULES, adapter_source, review_for

ROOT = Path(__file__).resolve().parent.parent


def identity(engine: dict) -> dict:
    return {
        "module": engine.get("engine", engine["name"]),
        "base_url": engine.get("base_url", ""),
        "search_url": engine.get("search_url", ""),
    }


def spacing_for(outgoing: dict, host: str) -> int:
    matches = [
        (domain, millis)
        for key, millis in outgoing.get("host_spacing_overrides", {}).items()
        if (domain := key.strip().lstrip(".").lower())
        and (host == domain or host.endswith("." + domain))
    ]
    return max(matches, key=lambda item: len(item[0]))[1] if matches else outgoing.get("host_spacing_ms", 0)


def validate(settings: dict, policy: dict, pending: dict) -> list[str]:
    errors = []
    allowlist = {name.strip().lower() for name in settings.get("search", {}).get("engine_allowlist", [])}
    for engine in settings["engines"]:
        if engine.get("inactive", False):
            continue
        if allowlist:
            if engine["name"].lower() not in allowlist and engine.get("engine", engine["name"]).lower() not in allowlist:
                continue
        elif engine.get("disabled", False):
            continue
        name = engine["name"]
        module = engine.get("engine", name)
        review = review_for(engine, policy)
        allowed = review.get("standard_allowed")
        if allowed is False:
            errors.append(f"{name}: {module} must be disabled in Standard ({review.get('decision')})")
        elif allowed is True:
            if review.get("status") != "reviewed_allowed_for_scope":
                errors.append(f"{name}: affirmative review status required")
            source = adapter_source(ROOT, module)
            if not source.exists() or hashlib.sha256(source.read_bytes()).hexdigest() != review.get("source_sha256"):
                errors.append(f"{name}: adapter changed; verify endpoint/use conditions and update its review")
            if module in GENERIC_MODULES and name not in policy.get("engines", {}):
                errors.append(f"{name}: generic module {module} needs an exact named endpoint review")
            if (name in policy.get("engines", {}) and review.get("module") != module) or review.get("module", module) != module:
                errors.append(f"{name}: reviewed module does not match the configured module")
            for field in ("base_url", "search_url"):
                if engine.get(field, "") != review.get(field, ""):
                    errors.append(f"{name}: configured {field} needs a separate endpoint review")
            for field in ("terms_url", "request_url", "access_method", "status", "decision", "conditions"):
                if not review.get(field):
                    errors.append(f"{name}: reviewed provider needs {field}")
            try:
                if date.fromisoformat(review.get("review_date", "")) > date.today():
                    raise ValueError("future review date")
            except (TypeError, ValueError):
                errors.append(f"{name}: valid non-future review_date required")
            if minimum := review.get("minimum_spacing_ms"):
                host = review["rate_limit_host"]
                if spacing_for(settings.get("outgoing", {}), host) < minimum:
                    errors.append(f"{name}: {host} requires at least {minimum} ms between requests")
        else:
            errors.append(f"{name}: Standard provider needs an affirmative permission review; a pending snapshot is not approval")
    return errors


def main() -> None:
    settings = tomllib.loads((ROOT / "settings.toml").read_text(encoding="utf-8"))
    policy = json.loads((ROOT / "docs/provider-policy.json").read_text(encoding="utf-8"))
    errors = validate(settings, policy, {})
    # Only the profiles that claim compliance are checked. The full profile
    # is the operator explicit opt-in to unreviewed sources and is not held
    # to this bar; its docs say so.
    for profile in ("clean",):
        overlay = tomllib.loads((ROOT / f"settings.{profile}.toml").read_text(encoding="utf-8"))
        engines = {entry["name"]: entry for entry in overlay.get("engines", [])}
        reviewed = {**settings, "engines": [{**entry, **engines.get(entry["name"], {})} for entry in settings["engines"]]}
        for section in ("search", "outgoing"):
            reviewed[section] = {**settings.get(section, {}), **overlay.get(section, {})}
            if section == "outgoing":
                reviewed[section]["host_spacing_overrides"] = {
                    **settings.get(section, {}).get("host_spacing_overrides", {}),
                    **overlay.get(section, {}).get("host_spacing_overrides", {}),
                }
        errors.extend(validate(reviewed, policy, {}))
    if errors:
        raise SystemExit("\n".join(errors))
    print("Standard policy checks passed: every enabled entry has a recorded conditional review.")


if __name__ == "__main__":
    main()
