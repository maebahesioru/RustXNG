# 第2バッチ: Standard プロバイダー規約の再取得（残り66エンジン）

第1バッチ（provider-terms-api.json、38エンジン）の続き。2026-11-05に一次資料から取得した審査提案で、人間が確認するまで許可の確定ではない。エンジンの実アクセス経路は src/engines/*.rs のURLリテラルから確認し、検証できたものだけ判定を提案した。

## 判定サマリ

| キー | 判定提案 | 状態 |
| --- | --- | --- |
| packagist | true | documented_api |
| wiby | true | documented_api_best_effort |
| repology | true | documented_api_with_rate_limit |
| apple_app_store | true | documented_api_with_terms |
| artic | true | documented_api_cc0 |
| iconify | true | documented_public_api |
| mixcloud | true | documented_api_with_rate_limits |
| hex | true | documented_api_docs_page_404 |
| gitea | true | documented_api_per_instance_terms |
| github_code | false | code_search_api_requires_auth |
| pexels | false | api_key_required |
| genius | false | restricted_internal_endpoint_and_commercial_licence |
| iqiyi | false | terms_prohibit_unauthorized_access |
| pixiv | false | prohibited_conduct_clause_present_scraping_item_unverified |
| currency_convert | false | restricted_internal_endpoint |
| … 審査継続 51件 | pending | 規約未確認・アクセス経路未検証など |

許可提案 (9件): packagist, wiby, repology, apple_app_store, artic, iconify, mixcloud, hex, gitea

無効提案 (6件): github_code, pexels, genius, iqiyi, pixiv, currency_convert

審査継続 (51件): トレント索引7件、中国の検索先4件、HTML取得35件、API移行候補を含む。詳細は provider-terms-batch2.json を参照。

## 許可提案の条件（要約）

### packagist

- 取得先: https://packagist.org/search.json?q={query}&page={pageno}
- 利用規約: https://packagist.org/apidoc
- 最小間隔: 1000 ms (packagist.org)
- Packagist APIは文書化されている (apidoc)
- 識別可能なUser-Agentとレート制限の遵守
- 大量取得はダンプの利用が推奨され得る

### wiby

- 取得先: https://wiby.me/json/?q={query}&p={pageno}
- 利用規約: https://wiby.me/
- 最小間隔: 1000 ms (wiby.me)
- wibyはJSON APIを公式に案内している
- 可用性の保証はなく、ベストエフォートの公開サービス
- 間隔は控えめに保つ

### repology

- 取得先: https://repology.org/api/v1/project/{query}
- 利用規約: https://repology.org/api
- 最小間隔: 1000 ms (repology.org)
- APIの許容レートは最大1リクエスト/秒 (公式API頁に明記)
- 一括 (1日1,000超) は推奨されず、データベースダンプの利用を検討
- 一括クライアントはUser-Agentでの自己識別が必須

### apple_app_store

- 取得先: https://itunes.apple.com/search (iTunes Search API)
- 利用規約: https://performance-partners.apple.com/search-api
- 最小間隔: 3000 ms (itunes.apple.com)
- iTunes Search APIは文書化され、規約が適用される
- ストアコンテンツを検索する目的の利用であり、エンターテインメント用途ではない
- サンプル等のアセットはストアバッジの近接表示が必要
- レート制限があり、過度な利用は制限され得る

### artic

- 取得先: https://api.artic.edu/api/v1/artworks/search
- 利用規約: https://api.artic.edu/docs/
- Art Institute of Chicago APIは文書化され、データはCC0
- レート制限とポリシーの遵守
- 画像の権利は個別に確認

### iconify

- 取得先: https://api.iconify.design/search
- 利用規約: https://iconify.design/docs/api/
- Iconifyの公開APIは文書化され、キー不要
- アイコンデータのライセンスは各セットに従う
- 適度な利用量に保つ

### mixcloud

- 取得先: https://api.mixcloud.com/search/?q={query}&type=cloudcast
- 利用規約: https://www.mixcloud.com/developers/
- Mixcloud APIは文書化され、レート制限がある
- 音声コンテンツの権利は権利者に帰属
- 埋め込みプレーヤー経由の再生が想定される利用方法

### hex

- 取得先: https://hex.pm/api/packages?search={query}
- 利用規約: https://hex.pm/docs/api
- 最小間隔: 1000 ms (hex.pm)
- hex.pm APIはCargo index標準のAPIで広く使われる
- hex.pm/docs/api は2026-11-05に404。公式ドキュメントの所在は要再確認
- 識別可能なUser-Agentとレート制限の遵守

### gitea

- 取得先: https://gitea.com/api/v1/repos/search?q={query}
- 利用規約: https://gitea.com/terms-of-use
- 最小間隔: 1000 ms (gitea.com)
- Gitea APIは各インスタンスのswagger (/api/swagger) で文書化される
- docs.gitea.comのAPI利用頁は2026-11-05に404。所在は要再確認
- インスタンスごとの利用規約・レート制限の確認が必要

## 無効提案の理由（要約）

- **github_code**: GitHubコード検索APIは認証必須 (キーなしでは常に失敗) / レート制限は検索で10リクエスト/分 / キーを設定した利用者の明示的な再有効化を優先
- **pexels**: Pexels APIはキー必須で、キーなしでは応答しない / エンジンはAPIではなくサイト検索 (HTML) を使っている / 移行先は公式API。帰属・ライセンス条項の確認が必要
- **genius**: genius.com/apiはサイト内部エンドポイント (公開APIドキュメントの対象外) / Genius APIの商用利用はライセンスなしでは認められない (docs.genius.com) / ライセンスの確認までStandardでは無効
- **iqiyi**: iQIYI利用規約は、事前の明示的な書面同意なしにプラットフォームのサービス・コンテンツ・データを取得することを禁止 / 自動取得の許諾確認までStandardでは無効
- **pixiv**: pixiv利用規約に禁止行為条項 (Prohibited Conduct) がある / 自動収集の該当項は人間が未確認 / ガイドラインが自動収集を制限している可能性 / 確認までStandardでは無効
- **currency_convert**: DuckDuckGo Spiceはサイト内部エンドポイントで、公開APIドキュメントの対象外 / 為替データの提供元は上流ソース / 内部エンドポイントの利用許諾確認までStandardでは無効

## 実装上の注意

- 文書化APIとエンジンの経路が一致していても、商用利用・保存・再配布の許諾は別に確認する
- 規約頁が403/404（JS描画）のものは到達可能性の記録であり、禁止の確定ではない
- pendingのままのエンジンは、既定無効の候補として扱う（provider-review-pending.json のスナップショットが継続）
