# Integration evaluation on 2026-11-06

The source commit is 45788f648bed15771d0cdf5f84eae1adb39fafbb.
The 100 gold URLs and existing local indexes were retained. Each coverage run started a fresh
process and response cache, with a 1.1-second delay between queries. The order was API, Standard, API.
The binary hash, settings hash and question hash are recorded in coverage/metadata.json.

Exa reached its credit limit during the first API run. A diagnostic request confirmed
HTTP 402 and NO_MORE_CREDITS. The later API and Japanese observations include this outage.
The runtime suspended that source and returned results from the remaining selected sources.

Summary.json separates HTTP endpoint errors from per-source availability.
The compressed coverage files retain every response. The Japanese corpus and selected engine
conditions are recorded in their own metadata. Adjacent Japanese repeats share the response cache.

The earlier 2026-11-05 API-active runs and frozen payload comparison remain in their original
directory. Issue #104 requires a funded-source verification before the integration can satisfy
the requested retrieval-quality gate.
