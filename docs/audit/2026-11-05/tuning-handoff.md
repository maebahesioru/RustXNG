# チューニングの引き継ぎ（2026-11-06、lead → Codex）

settings.api.toml には weight がない（exaapi 1.0）。100問で測った知見:

- **exaapi weight 2.0 + relevance_weight 2.0** が最良の既知構成: 79/92/0.849（api-1 の 72/88/0.791 より上）
- 同じ構成の再現: 77/92/0.839。差は上流変動（±1〜2問）の範囲
- 根拠データ: docs/audit/2026-11-05/exa-tuning-2026-11-05.md（v1〜v13 のグリッド）
- 注意: tavily は条項確認が未完了（審査未完了のまま）。settings.api.toml への採用は確認後が安全
- 遵守の仕組み: settings.toml の allowlist + checker（全有効エントリに記録済みの条件付き審査）+ 在庫一致 — いまは通っている
