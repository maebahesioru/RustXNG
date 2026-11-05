# Standard プロバイダー規約・API条件の再取得（2026-11-05）

最新ベース（v0.28.0系マージ済み + PR 1〜3の作業ツリー）のStandardで有効な未審査プロバイダーのうち、
API系を中心に 38 エンジン（27 モジュール/名前キー）分の利用条件を一次資料から取得した。
**本書は審査提案であり、人間が一次資料を確認するまで許可の確定ではない**（計画書「監査状態は自動判定で捏造しない」に従う）。

## 判定サマリ

| キー | 判定提案 | 状態 |
| --- | --- | --- |
| wikipedia | true | documented_api_with_conditions |
| wikipedia_entities | true | documented_api_with_conditions |
| wikipedia_any | true | documented_api_with_conditions |
| wikidata | true | documented_api_with_conditions |
| wikicommons | true | documented_api_with_conditions |
| mediawiki (12エンジン) | true | documented_api_with_conditions |
| github | true | documented_api_with_rate_limits |
| stackexchange | true | documented_api_with_attribution |
| crates | true | documented_api_with_rate_limit |
| pypi | false | html_search_permission_unconfirmed |
| arxiv | true | documented_api_with_rate_limit |
| crossref | true | documented_api_polite_pool |
| cinii | false | api_registration_required |
| semantic_scholar | true | documented_api_with_rate_limits |
| core | false | licence_required_for_api_and_products |
| openalex | true | documented_api_with_credits |
| pubmed | true | documented_api_with_usage_guidelines |
| open_meteo | true | conditional_api_non_commercial |
| photon | true | public_instance_best_effort |
| openverse | true | documented_api_low_anonymous_tier |
| dailymotion | true | documented_api_with_rate_limits |
| deezer | false | api_terms_behind_login |
| vimeo | false | internal_endpoint_and_auth_unclear |
| docker_hub | false | terms_page_not_machine_readable |
| mdn | false | restricted_internal_endpoint |
| microsoft_learn | false | restricted_internal_endpoint |
| npm | pending | review_incomplete_terms_not_located |

許可提案 (true): wikipedia, wikipedia_entities, wikipedia_any, wikidata, wikicommons, mediawiki, github, stackexchange, crates, arxiv, crossref, semantic_scholar, openalex, pubmed, open_meteo, photon, openverse, dailymotion

無効提案 (false): pypi, cinii, core, deezer, vimeo, docker_hub, mdn, microsoft_learn

審査継続 (pending): npm

## 各プロバイダーの詳細

### wikipedia

- 取得先: https://{language}.wikipedia.org/w/api.php
- 方式: MediaWiki Action API
- 利用規約: https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use
- ドキュメント: https://www.mediawiki.org/wiki/API:Etiquette
- Wikimedia利用規約: 本文はCC BY-SA 4.0等の自由ライセンスで提供され、再利用時の帰属が必要
- MediaWiki API etiquette: 読み取り専用リクエストにもレート制限がある。連絡先付きの識別可能なUser-Agentが必須で、ブラウザUAの偽装は禁止
- Wikimedia User-Agentポリシー: 識別できないクライアントは予告なくブロックされ得る
- RustXNGはWikimedia系宛てに識別可能なUser-Agent (rustxng/<版> (+URL[; 連絡先])) を既に送信している (docs/PROVIDERS.md 2026-11-03)

### wikipedia_entities

- 取得先: https://{language}.wikipedia.org/w/api.php (entity search variant)
- 方式: MediaWiki Action API
- 利用規約: https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use
- ドキュメント: https://www.mediawiki.org/wiki/API:Etiquette
- Wikimedia利用規約: 本文はCC BY-SA 4.0等の自由ライセンスで提供され、再利用時の帰属が必要
- MediaWiki API etiquette: 読み取り専用リクエストにもレート制限がある。連絡先付きの識別可能なUser-Agentが必須で、ブラウザUAの偽装は禁止
- Wikimedia User-Agentポリシー: 識別できないクライアントは予告なくブロックされ得る
- RustXNGはWikimedia系宛てに識別可能なUser-Agent (rustxng/<版> (+URL[; 連絡先])) を既に送信している (docs/PROVIDERS.md 2026-11-03)

### wikipedia_any

- 取得先: https://{language}.wikipedia.org/w/api.php (any-words variant)
- 方式: MediaWiki Action API
- 利用規約: https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use
- ドキュメント: https://www.mediawiki.org/wiki/API:Etiquette
- Wikimedia利用規約: 本文はCC BY-SA 4.0等の自由ライセンスで提供され、再利用時の帰属が必要
- MediaWiki API etiquette: 読み取り専用リクエストにもレート制限がある。連絡先付きの識別可能なUser-Agentが必須で、ブラウザUAの偽装は禁止
- Wikimedia User-Agentポリシー: 識別できないクライアントは予告なくブロックされ得る
- RustXNGはWikimedia系宛てに識別可能なUser-Agent (rustxng/<版> (+URL[; 連絡先])) を既に送信している (docs/PROVIDERS.md 2026-11-03)

### wikidata

- 取得先: https://www.wikidata.org/w/api.php
- 方式: MediaWiki Action API
- 利用規約: https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use
- ドキュメント: https://www.mediawiki.org/wiki/API:Etiquette
- Wikimedia利用規約: 本文はCC BY-SA 4.0等の自由ライセンスで提供され、再利用時の帰属が必要
- MediaWiki API etiquette: 読み取り専用リクエストにもレート制限がある。連絡先付きの識別可能なUser-Agentが必須で、ブラウザUAの偽装は禁止
- Wikimedia User-Agentポリシー: 識別できないクライアントは予告なくブロックされ得る
- RustXNGはWikimedia系宛てに識別可能なUser-Agent (rustxng/<版> (+URL[; 連絡先])) を既に送信している (docs/PROVIDERS.md 2026-11-03)

### wikicommons

- 取得先: https://commons.wikimedia.org/w/api.php
- 方式: MediaWiki Action API
- 利用規約: https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use
- ドキュメント: https://www.mediawiki.org/wiki/API:Etiquette
- Wikimedia利用規約: 本文はCC BY-SA 4.0等の自由ライセンスで提供され、再利用時の帰属が必要
- MediaWiki API etiquette: 読み取り専用リクエストにもレート制限がある。連絡先付きの識別可能なUser-Agentが必須で、ブラウザUAの偽装は禁止
- Wikimedia User-Agentポリシー: 識別できないクライアントは予告なくブロックされ得る
- RustXNGはWikimedia系宛てに識別可能なUser-Agent (rustxng/<版> (+URL[; 連絡先])) を既に送信している (docs/PROVIDERS.md 2026-11-03)

### mediawiki

- 取得先: per-engine base_url (wiki.nixos.org, wiki.gentoo.org, minecraft.wiki, directory.fsf.org, {language}.wikibooks.org ほか)
- 方式: MediaWiki Action API (第三者MediaWikiインスタンス)
- 利用規約: https://www.mediawiki.org/wiki/API:Etiquette
- ドキュメント: https://www.mediawiki.org/wiki/API:Main_page
- MediaWiki API etiquette: 読み取り専用リクエストにもレート制限がある。連絡先付きの識別可能なUser-Agentが必須で、ブラウザUAの偽装は禁止
- Wikimedia User-Agentポリシー: 識別できないクライアントは予告なくブロックされ得る
- RustXNGはWikimedia系宛てに識別可能なUser-Agent (rustxng/<版> (+URL[; 連絡先])) を既に送信している (docs/PROVIDERS.md 2026-11-03)
- 第三者MediaWiki (NixOS/Gentoo/Minecraft wiki, FSF directory等) は各サイトの利用規約が別途ある。APIはMediaWiki標準機能だが、サイトごとの確認を推奨

### github

- 取得先: https://api.github.com/search/repositories
- 方式: Documented REST API (unauthenticated)
- 利用規約: https://docs.github.com/en/site-policy/github-terms/github-terms-of-service
- ドキュメント: https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api
- GitHub ToSにはAPI専用の条項があり、開発・データ収集のためのAPI利用を定める
- 未認証リクエストは60回/時 (コア)。検索エンドポイントはさらに低い別枠 (10回/分)
- Acceptable Useポリシーは同意なきWebサイトのスクレイピングを禁止。エンジンはHTMLではなく文書化されたAPIを使う
- 結果メタデータの再利用はGitHub ToSと各リポジトリのライセンスに従う

### stackexchange

- 取得先: https://api.stackexchange.com/2.3/search/advanced
- 方式: Documented REST API
- 利用規約: https://stackoverflow.com/legal/api-terms-of-use
- ドキュメント: https://api.stackexchange.com/docs
- API利用規約: アプリはStack Exchange Networkを情報源として明示しなければならない
- 検索エンジンに索引されるアプリはStack Exchange ToSの帰属規則に従う必要がある
- APIはクォータ/スロットリング対象。本番利用ではAPIキーを推奨
- 投稿本文はCC BY-SA。全文再利用はライセンスの帰属要件に従う

### crates

- 取得先: https://crates.io/api/v1/crates
- 方式: Documented JSON API
- 利用規約: https://crates.io/data-access
- ドキュメント: https://crates.io/data-access
- 最小間隔: 1000 ms (crates.io)
- crates.ioデータアクセス方針: sparse indexとダンプを優先し、APIは公表された制限内で利用してよい
- API利用には連絡先付きのUser-Agent識別と最大1リクエスト/秒が要求される
- 制限超過時は429を返すため、429時はバックオフする

### pypi

- 取得先: https://pypi.org/search/?q={query}
- 方式: HTML search endpoint (not the documented JSON API)
- 利用規約: https://policies.python.org/pypi.org/Terms-of-Service/
- ドキュメント: https://pypi.org/user-guide/
- エンジンはPyPIのHTML検索頁 (https://pypi.org/search/) を使う。PyPI ToSのAPI条項はAPI利用を律するもので、HTML経路はその対象外
- PyPI ToS: API経由の乱用・過度に頻繁なリクエストはAPIアクセス停止になり得る。アカウントのbot登録は禁止
- Webサイト検索頁への自動アクセスについて明示的な許可は2026-11-05時点で確認できていない
- PyPIに公式のJSON検索APIはなく (XMLRPC APIは非推奨)、移行先が無いため、許諾確認までStandardでは無効を提案

### arxiv

- 取得先: https://export.arxiv.org/api/query
- 方式: Documented Atom API
- 利用規約: https://info.arxiv.org/help/api/tou.html
- ドキュメント: https://info.arxiv.org/help/api/
- 最小間隔: 3000 ms (export.arxiv.org)
- arXiv API利用規約: 3秒に1回以内、同時1接続 (管理下の全マシン合算)
- 電子プリント (PDF/ソース) は著作権者の許可なく保存・配信しない。検索用メタデータは対象外
- arXiv行動規範に従う。違反時はAPI利用を禁止され得る

### crossref

- 取得先: https://api.crossref.org/works
- 方式: Documented REST API (polite pool)
- 利用規約: https://www.crossref.org/documentation/retrieve-metadata/rest-api/
- ドキュメント: https://www.crossref.org/documentation/retrieve-metadata/rest-api/
- Crossref REST API: mailtoパラメータ (連絡先) を付けてpolite poolに入れることが推奨
- 書誌メタデータは事実データ。APIガイドラインとレート制限ヘッダに従う
- 大量・商用利用にはPlus登録が必要になり得る

### cinii

- 取得先: https://cir.nii.ac.jp/opensearch/all
- 方式: OpenSearch API
- 利用規約: https://support.nii.ac.jp/en/cinii/api/developer
- ドキュメント: https://support.nii.ac.jp/en/cinii/api/api_outline
- NIIはOpenSearch/RDF等のAPI利用に、利用登録 (アプリケーションID取得) を求める
- 「学術コンテンツサービス利用規約」「CiNii Research利用細則」「ウェブAPI利用細則」への同意が必要
- CiNii Booksの書誌データはCC BY 4.0オープンデータ。その他のコンテンツの権利は個別
- 登録と細則確認が済むまでStandardでは無効にする

### semantic_scholar

- 取得先: https://api.semanticscholar.org/graph/v1/paper/search
- 方式: Documented JSON API
- 利用規約: https://www.semanticscholar.org/product/api
- ドキュメント: https://api.semanticscholar.org/api-docs/
- Semantic Scholar APIは無料。未認証リクエストは共有プールでスロットリングされ得る
- APIキーを推奨 (入門枠 1 RPS)。キーの共有は禁止
- データ出典をSemantic Scholarに帰属 (S2論文の引用) し、全文の権利は元論文のライセンスに従う
- 大量取得はAPIではなくS2AGデータセット配布を使う

### core

- 取得先: https://api.core.ac.uk/v3/search/works/
- 方式: Documented JSON API (licence required)
- 利用規約: https://core.ac.uk/terms
- ドキュメント: https://core.ac.uk/services/api
- CORE利用条件: CORE APIの利用にはライセンスが必要。無料ライセンスもあるが適格性の確認が要る
- 収益化の可能性や、CORE自身のサービス (API・推薦・検索・発見システム等) に類似する機能を持つ製品は、COREへの連絡が求められる
- COREはrobots.txtとアクセス制御を尊重して収集しているが、個別コンテンツのライセンスは別途確認が必要
- Standardではライセンス適格性の確認まで無効にする

### openalex

- 取得先: https://api.openalex.org/works
- 方式: Documented REST API
- 利用規約: https://docs.openalex.org/
- ドキュメント: https://github.com/ourresearch/openalex-docs/blob/main/how-to-use-the-api/rate-limits-and-authentication.md
- OpenAlexは無料で使える。無料ユーザーの上限は100,000クレジット/日と100リクエスト/秒。list系エンドポイント (/works?filter= 等) は1リクエスト10クレジットを消費する
- mailtoを付けるとpolite poolに入り、安定した応答になる
- OpenAlexのデータはCC0。帰属は義務ではないが推奨される
- 検索トラフィックには100,000クレジット/日は十分な枠

### pubmed

- 取得先: https://eutils.ncbi.nlm.nih.gov/entrez/eutils
- 方式: Documented E-utilities API
- 利用規約: https://www.ncbi.nlm.nih.gov/books/NBK25497/
- ドキュメント: https://eutilities.github.io/site/API_Key/usageandkey/
- 最小間隔: 334 ms (eutils.ncbi.nlm.nih.gov)
- NCBI E-utilities利用指針: 3 URLリクエスト/秒以内
- 大きなジョブ (100リクエスト超) は週末または米東部時間 21:00–05:00 に。APIキーで上限引き上げ可
- 抄録・索引データが中心。全文の著作権は出版者に帰属
- NBK25497は2026-11-05時点で自動取得ができなかったため、NCBI公式のE-utilities文書サイト (eutilities.github.io) で指針を確認した

### open_meteo

- 取得先: https://api.open-meteo.com/v1/forecast
- 方式: Documented JSON API (keyless)
- 利用規約: https://open-meteo.com/en/terms/
- ドキュメント: https://open-meteo.com/en/docs/
- Open-Meteo利用規約: 無料APIは非商用目的のみ。商用利用はサブスクリプションが必要
- 無料枠: 10,000呼び出し/日未満、5,000/時、600/分 (IPおよびアプリケーション単位)
- CC BY 4.0の帰属 (Open-Meteoへのクレジット) が必要
- 乱用は予告なくブロックされ得る。サーバーレス環境では早期に制限がかかる場合がある

### photon

- 取得先: https://photon.komoot.io/api/
- 方式: Public geocoding API (open-source instance)
- 利用規約: https://photon.komoot.io/
- ドキュメント: https://github.com/komoot/photon
- photonの公開インスタンス: 大量利用はスロットリングまたは全面禁止になり得る。可用性の保証はなく、予告なく変更され得る
- 大量リクエストには自前インスタンスの運用が推奨されている
- データは © OpenStreetMap contributors (ODbL)。結果表示時の帰属が必要
- リクエスト量は控えめに保つ

### openverse

- 取得先: https://api.openverse.org/v1/images/
- 方式: Documented JSON API
- 利用規約: https://docs.openverse.org/api/reference/authentication_and_throttling.html
- ドキュメント: https://api.openverse.org/v1/
- 最小間隔: 3000 ms (api.openverse.org)
- Openverse APIの匿名 (未登録) ティア: バースト20リクエスト/分、持続200リクエスト/日 (APIのレート制限ヘッダで確認。ドキュメント頁はJS描画のため本文を取得できなかった)
- 上限引き上げにはアプリケーション登録 (standardティア) が必要
- 検索結果は各コンテンツのライセンス (CC等) に従い帰属が必要
- 共有インスタンスでは200/日は少ないため、429を監視する

### dailymotion

- 取得先: https://api.dailymotion.com/videos
- 方式: Documented JSON API
- 利用規約: https://legal.dailymotion.com/en/terms-of-use/
- ドキュメント: https://developers.dailymotion.com/api/
- Dailymotion APIは文書化されており、プラットフォームの安定と公平な利用のためレート制限を実施する
- Dailymotion利用規約がサービスを律する。動画コンテンツの権利は権利者に帰属
- 検索結果はメタデータであり、視聴はDailymotionプレーヤー経由が想定される利用方法

### deezer

- 取得先: https://api.deezer.com/search
- 方式: Public JSON API (simple API)
- 利用規約: https://developers.deezer.com/api/terms
- ドキュメント: https://developers.deezer.com/api
- DeezerのAPI利用規約は開発者ログインの先で提示される (「simple APIの利用規約に同意するにはログインが必要」)
- 2026-11-05時点で規約本文を公開状態で確認できなかった。有効化前に規約の確認と記録が必要
- 返却されるトラックメタデータは統合目的。コンテンツの権利は権利者に帰属

### vimeo

- 取得先: https://api.vimeo.com (guest JWT from https://vimeo.com/_next/jwt)
- 方式: API with internal JWT bootstrap
- 利用規約: https://vimeo.com/terms
- ドキュメント: https://developer.vimeo.com/api/guides/start
- エンジンは検索前に vimeo.com/_next/jwt (サイト内部エンドポイント) からゲストJWTを取得している
- Vimeo APIは通常アクセストークン認証が必要で、未認証利用は文書化されたAPIの一部ではない
- Vimeo利用規約がサービスを律する。内部エンドポイントの利用は公開API規約でカバーされない
- アクセス経路の確認または資格情報の設定までStandardでは無効にする

### docker_hub

- 取得先: https://hub.docker.com (website search endpoint)
- 方式: Undocumented website search endpoint
- 利用規約: https://www.docker.com/legal/docker-terms-use
- ドキュメント: https://docs.docker.com/docker-hub/api/
- エンジンはhub.docker.comのサイト検索を呼び出しているが、これはDockerの文書化された公開APIではない
- Docker利用規約ページはJavaScript描画で、2026-11-05には条項本文を自動取得できなかった。人間による確認が必要
- Docker Hubにはレジストリ操作の認証付きAPIがあるが、検索の匿名利用は許可として文書化されていない
- 利用規約の確認が済むまでStandardでは無効にする

### mdn

- 取得先: https://developer.mozilla.org/api/v1/search?q={query}&page={pageno}
- 方式: Internal JSON endpoint (undocumented)
- 利用規約: https://www.mozilla.org/en-US/about/legal/terms/mozilla/
- MDNの /api/v1/search はMDNサイト自身の検索用で、公開APIドキュメントが見つからない
- MDNコンテンツはCC BY-SAだが、アクセス経路そのものが内部エンドポイント
- Mozillaの利用規約がサイトを律する。内部エンドポイント利用は許可として文書化されていない
- 文書化されたアクセス経路への移行までStandardでは無効にする

### microsoft_learn

- 取得先: https://learn.microsoft.com/api/search
- 方式: Internal JSON endpoint (undocumented)
- 利用規約: https://learn.microsoft.com/en-us/legal/
- learn.microsoft.com/api/search はMicrosoft Learnサイト自身の検索用で、公開APIドキュメントが見つからない
- Microsoftの利用規約がサービスを律する。内部エンドポイント利用は許可として文書化されていない
- Microsoft Learnのコンテンツは個別の著作権表示・ライセンス (一部CC BY) に従う
- 文書化されたアクセス経路への移行までStandardでは無効にする

### npm

- 取得先: https://api.npms.io/v2/search
- 方式: Third-party JSON API (npms.io)
- 利用規約: https://npms.io/
- ドキュメント: https://github.com/npms-io/npms-analyzer
- npms.ioはnpmjs.comではなく第三者のnpm検索。サイトはJavaScript描画で、2026-11-05には公開API規約が見つからなかった
- npms.ioはオープンソース (npms-analyzer) だが、明示的な許可も禁止も確認できていない
- 審査未完了のため standard_allowed は意図的に空にする (pendingとして維持)
- npmjs.com自体のレジストリ規約は第三者npms.io APIを自動的にはカバーしない

## 汎用モジュールのキーについて

provider-policy.json はモジュール名で照合するため、汎用モジュール（json_engine、xpath）を使うエンジン
（mdn、packagist、wiby、bitbucket、etymonline、hoogle、pub.dev、rubygems、woxikon）は名前単位の扱いが必要。
本書の mdn はエンジン名キー。mediawiki は12エンジン共通の1キーで整理した。

## 未カバー（第2バッチで取得、66エンジン）

残りの66エンジンは第2バッチとして取得した。判定サマリと条件は [provider-terms-batch2.md](provider-terms-batch2.md)、データは provider-terms-batch2.json を参照。対象は次のとおり。

500px, 9gag, alpinelinux, apkmirror, apple_app_store, artic, bitbucket, bitchute, bpb, cachy_os, ccc_media, currency_convert, deviantart, dictzone, duden, emojipedia, etymonline, fdroid, findfiles, flaticon, geizhals, genius, gitea, github_code, gitlab, hex, hoogle, iconify, imgur, iqiyi, jisho, kickass, lemmy, lib_rs, mastodon, mixcloud, niconico, nyaa, odysee, openclipart, packagist, peertube, pexels, piratebay, pixiv, pkg_go_dev, pub.dev, r1337x, r360search_videos, repology, rubygems, rumble, sepiasearch, shopify_stock, sogou_images, sogou_videos, solidtorrents, sourcehut, steam, tokyotoshokan, tootfinder, wallhaven, wiby, wordnik, woxikon.de synonyme, www1x
