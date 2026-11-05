# 2026-11-05 監査ファイル一覧

計画書（RustXNG_Standard_Provider_Compliance_Plan.md）のPR 4（回帰測定）と、Standardの未審査プロバイダー104件の規約再取得の記録。Codex（codex/standard-provider-performanceブランチ）と分割して作業した。

## 規約・API条件の審査提案

| ファイル | 内容 |
| --- | --- |
| provider-terms-api.md | 第1バッチのレポート。API系38エンジン（27キー）分。許可提案18・無効提案8・審査継続1 |
| provider-terms-api.json | 第1バッチのデータ。provider-policy.json と同じスキーマ（マージ用） |
| provider-terms-batch2.md | 第2バッチのレポート。残り66エンジン分。許可提案9・無効提案6・審査継続51 |
| provider-terms-batch2.json | 第2バッチのデータ |

どちらも審査提案で、人間が一次資料を確認するまで許可の確定ではない。マージは docs/provider-policy.json へ、確認後に行う。

## 実測データ

| ファイル | 内容 |
| --- | --- |
| standard-baseline-2026-11-05.md | 変更前後の100問比較レポート（3者比較） |
| rustxng-standard-quality-before.json | 変更前構成（11系統有効・119エンジン）の実測 |
| rustxng-standard-quality-after.json | 変更後（旧バイナリ・108エンジン）の実測（Codex測定） |
| sealqa-baseline-2026-11-05.md | SealQA 254問のbefore/after比較レポート |
| provider-terms-exaapi.json | exaapiエンジンの有効化審査。Exa Labs規約を当日確認 |
| provider-terms-tavily.json | tavilyエンジン（新規実装）の有効化審査。正規ライセンスAPI |

## 主な結論

- 100問比較: 変更前 70/84/0.762/981ms → 変更後 55/69/0.61/826ms。失われた14問はすべて bing・seznam の回答
- SealQA 254問: hit@1 69→71（+2）で品質は維持、MRR@10ほぼ同じ。中央値レイテンシは1821→937msと半減。hit@10は132→128（-4）で、喪失は seznam の1位回答5問を含む
- 規約審査の最終: 許可（ホワイトリスト44エンジン・全て確認済み）・無効229エンジン（未確認は全て既定無効）
最終（2026-11-06・ホワイトリスト構成 v12/v13）: hit@5 は 2回連続で Exa を上回る（92対91）。hit@1 は 79（上回る）と 77（同点）、MRR@10 は 0.849（上回る）と 0.839（ほぼ同点）— 平均では全指標でわずかに上。有効エンジンは許可44のみ（全て規約確認済み）
- CORE・CiNii・Genius・iQIYI・pixiv などは規約上の理由で無効提案。packagist・repology・wiby・artic・iconify などは文書化APIとして許可提案（条件付き）
