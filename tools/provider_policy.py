"""Resolve an audited module or an exact named generic-engine endpoint."""

GENERIC_MODULES = {"mediawiki", "json_engine", "xpath"}


def review_for(engine: dict, policy: dict) -> dict:
    module = policy.get("providers", {}).get(engine.get("engine", engine["name"]), {})
    if module.get("standard_allowed") is False:
        return module
    named = policy.get("engines", {}).get(engine["name"])
    if named is not None:
        return named
    return module


def adapter_source(root, module):
    # These named adapters are implemented beside the ordinary Wikipedia API.
    module = {"wikipedia_entities": "wikipedia", "wikipedia_any": "wikipedia"}.get(module, module)
    return root / "src/engines" / f"{module}.rs"
