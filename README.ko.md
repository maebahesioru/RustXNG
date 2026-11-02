# RustXNG

**AI 에이전트에게: RustXNG를 설치하고 로컬 웹 검색 도구로 사용하세요.**

명령을 실행할 수 있는 에이전트에게 아래 지시문을 붙여 넣거나 이 README를 전달하세요.

> <https://github.com/sahenjp/RustXNG>의 RustXNG를 설치하고 웹 검색에 사용하세요.
> 먼저 이미 실행 중인 RustXNG 서비스가 있는지 확인하고, 있으면 재사용하세요. 없으면 OS와 CPU를 확인해
> 맞는 릴리스 바이너리를 내려받고 SHA-256 체크섬을 검증한 뒤 사용자 소유 디렉터리에 설치하세요.
> [에이전트 설치 가이드](https://github.com/sahenjp/RustXNG/blob/main/docs/AGENT-SETUP.md)를 따르세요.
> loopback에서 실행하고 `/healthz`를 확인한 다음 `max_results=5`로 JSON 검색이 되는지 확인하세요.
> 첫 응답은 짧게 유지하고, 근거가 필요할 때만 페이지 본문을 요청하세요.
> 바이너리 버전, 접속 주소, 동작한 검색 명령 하나를 보고하세요. 설치에 실패하면 실제 오류를 보고하세요.

[English](README.md) · [日本語](README.ja.md) · [에이전트 설치 가이드](docs/AGENT-SETUP.md)

[![CI](https://github.com/sahenjp/RustXNG/actions/workflows/ci.yml/badge.svg)](https://github.com/sahenjp/RustXNG/actions/workflows/ci.yml)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)

## RustXNG를 쓰는 이유

RustXNG는 SearXNG를 바탕으로 Rust로 작성한 셀프 호스팅 메타검색 엔진입니다.
여러 출처의 결과를 합치고, 근거 확인(grounding)을 위해 페이지 본문을 가져올 수 있습니다.
에이전트는 필요한 만큼만 정보를 가져와 모델에 보내는 텍스트 양을 작게 유지할 수 있습니다.

많은 엔진은 API 키 없이 동작합니다. 인증이 필요한 선택적 제공자는 각자의 자격 증명과 요금 체계를 따릅니다.
서버는 로컬에서 실행되지만, **검색어는 요청마다 선택된 업스트림 엔진으로 전송됩니다**.

## 에이전트에서 검색하기

이미 사용할 수 있는 서비스가 없다면 설치한 바이너리를 실행합니다.

```bash
SEARXNG_PORT=29483 rustxng --bind 127.0.0.1 --port 29483
```

다른 터미널에서 결과를 최대 5개까지 요청합니다.

```bash
curl --fail --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=tokio runtime documentation' \
  --data 'format=json' --data 'max_results=5'
```

각 결과에는 `title`, `url`, `content`가 있습니다. 응답에는 제공자와 결과 메타데이터도 들어 있습니다.
이 세 필드만 모델에 넘기려면 로컬에서 `jq`로 추려 냅니다.

```bash
curl --fail --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=tokio runtime documentation' \
  --data 'format=json' --data 'max_results=5' \
  | jq '{results: [.results[] | {title, url, content}]}'
```

다음 매개변수로 응답 범위를 좁힐 수 있습니다.

| 매개변수 | 용도 |
| --- | --- |
| `max_results=5` | 순위가 매겨진 결과를 최대 5개 반환합니다. 지원 범위: 1–200 |
| `include_domains=docs.rs` | 이 도메인과 하위 도메인의 결과만 남깁니다 |
| `exclude_domains=example.com` | 이 도메인과 하위 도메인의 결과를 제외합니다 |
| `include_raw_content=1` | 상위 결과의 페이지 본문을 `raw_content`로 가져옵니다 |
| `engines=wikipedia` | 등록된 엔진을 선택합니다 |
| `time_range=day` | 이 필터를 지원하는 엔진에 최근 결과를 요청합니다 |

페이지 본문은 선택 사항입니다. 기본 설정에서는 결과 최대 5개에 대해,
페이지당 8,000자 한도로 본문을 가져옵니다. 개별 본문 가져오기는 실패할 수 있습니다.
문자 수 한도는 토큰 한도가 아닙니다. 아래 MCP 도구는 직렬화 기준 문자·바이트 예산을 추가로 제공합니다.
토큰 기준 예산과 HTTP API의 필드 선택은 계획 중입니다.

브라우저 UI는 <http://127.0.0.1:29483/>에 있습니다. JSON, CSV, RSS, HTML 출력을 지원합니다.

### MCP 서버

`rustxng mcp`를 실행하면 읽기 전용 도구인 `search`와 `read_page`를 stdio로 제공합니다.
search는 순위가 매겨진 발췌문과 출처 메타데이터를 반환합니다. 페이지 본문은
같은 세션의 검색이 반환한 URL로 `read_page`를 호출할 때만 가져옵니다.
search는 기본적으로 결과 5개를 반환하고 발췌문마다 500자로 제한합니다.
직렬화된 구조화 응답은 기본적으로 유니코드 12,000자, UTF-8 24,000바이트로 제한되며,
호출자는 공개된 범위 안에서 더 작거나 큰 한도를 고를 수 있습니다. 서버는 먼저 발췌문을 줄이고,
그다음 뒤쪽 결과를 생략하되 반환하는 결과의 URL과 메타데이터는 온전히 유지합니다. `truncated`,
`source_truncated`, `next_offset`은 이어서 요청할지 알려 줍니다. `read_page`는
유니코드 문자 오프셋을 쓰고 같은 잘림 상태를 보고합니다. `read_page`에
`query`를 함께 주면 일치하는 단락도 최대 3개 반환합니다.

페이지 읽기는 DNS 응답과 리다이렉트가 공개 주소인지 검사할 수 있도록 직접 연결을 요구합니다.
설정이나 환경 변수에 나가는 프록시가 있으면 안전하게 실패(fail closed)합니다.
검색은 계속 일반 프록시 설정을 사용합니다. 문자·바이트 한도는 직렬화된 구조화 JSON 페이로드만 셉니다.
JSON-RPC 봉투, SDK의 텍스트 렌더링, 호출자 프롬프트는 포함하지 않으며,
특정 토크나이저 기준의 토큰 수를 보장하지 않습니다.

## 설치

### 1. 릴리스 바이너리

[Releases](https://github.com/sahenjp/RustXNG/releases)에서 바이너리와 `.sha256` 파일을 함께 고릅니다.
공개된 v0.26.0 릴리스에는 Linux x86_64(GNU, musl), macOS arm64/x86_64,
Windows x86_64 바이너리가 있습니다. v0.26.0 이후 릴리스에는 Linux ARM64(`aarch64-unknown-linux-gnu`, glibc 2.39 이상)가 추가됩니다.
선택과 검증 방법은 [에이전트 설치 가이드](docs/AGENT-SETUP.md)에 있습니다.
미리 빌드된 바이너리는 Rust 툴체인이 필요 없습니다.

GitHub CLI와 사용자 소유 설치 디렉터리를 쓰는 Linux x86_64 예시입니다.

```bash
set -eu
mkdir -p rustxng-download && cd rustxng-download
gh release download v0.26.0 --repo sahenjp/RustXNG \
  --pattern 'rustxng-v0.26.0-x86_64-unknown-linux-musl*' \
  --pattern LICENSE --pattern NOTICE
sha256sum --check rustxng-v0.26.0-x86_64-unknown-linux-musl.sha256
install -Dm755 rustxng-v0.26.0-x86_64-unknown-linux-musl "$HOME/.local/bin/rustxng"
mkdir -p "$HOME/.local/share/rustxng"
cp LICENSE NOTICE "$HOME/.local/share/rustxng/"
"$HOME/.local/bin/rustxng" --version
```

`~/.local/bin`이 `PATH`에 없으면 바이너리의 절대 경로를 쓰거나 셸 설정에 이 디렉터리를 추가하세요.

### 2. Cargo

최신 stable Rust 툴체인과 C 빌드 툴체인을 사용합니다.

```bash
cargo install --git https://github.com/sahenjp/RustXNG.git --tag v0.26.0 --locked
```

`rustxng`와 `rustxng-index`가 설치됩니다.
최소 지원 버전은 Rust 1.88이며 CI에서 확인합니다. 이 체크아웃은 Rust 1.99.1에서도 검증했습니다.

### 3. Docker

저장소에서 이미지를 빌드합니다.

```bash
git clone https://github.com/sahenjp/RustXNG.git
cd RustXNG
docker build -t rustxng .
docker run --rm -p 127.0.0.1:29483:29483 rustxng
```

공개된 이미지(내부 기본 포트가 8888인 이전 이미지 포함)는 다음과 같이 실행합니다.

```bash
docker run --rm -p 127.0.0.1:29483:29483 -e SEARXNG_PORT=29483 ghcr.io/sahenjp/rustxng:latest --bind 0.0.0.0 --port 29483
```

다른 인터페이스에 공개하는 것은 명시적인 배포 선택입니다. 그에 맞게 인증·접근 제어를 설정하세요.
설정을 넘기려면 이미지 이름 앞에 `-v "$PWD/settings.toml:/app/settings.toml:ro"`로
[settings.toml](settings.toml) 전체 사본을 마운트합니다. 컨테이너 인수를 바꿀 때는
`--bind 0.0.0.0`과 `--port 29483`을 모두 넣으세요. 공개 이미지는 현재 Linux amd64만 대상으로 합니다.
다른 아키텍처에서는 로컬에서 빌드하거나 해당 네이티브 바이너리를 사용하세요.

### 4. 소스

```bash
git clone https://github.com/sahenjp/RustXNG.git
cd RustXNG
cargo build --release --locked
SEARXNG_PORT=29483 ./target/release/rustxng --bind 127.0.0.1 --port 29483
```

Rust 1.88 이상, 가능하면 최신 stable 툴체인을 사용하세요.

## 설정과 운영

이 체크아웃의 기본 주소는 `127.0.0.1:29483`입니다. 공개된 v0.26.0 바이너리는 아직 기본 포트가 8888이지만,
위의 `--port 29483` 명령은 두 버전 모두에서 동작합니다.
rdsh 설정을 포함한 내용은 [포트 이전과 선택](docs/PORT-MIGRATION.md)을 참고하세요.

기본값은 바이너리에 내장되어 있습니다. 설정 파일을 지정하려면 다음과 같이 실행합니다.

```bash
rustxng --config /path/to/settings.toml
```

[settings.toml](settings.toml)과 [설정 상세](docs/PORTING.md)를 참고하세요.
부분 설정 병합에는 지정하지 않은 값이 초기화될 수 있는 알려진 문제가 있습니다. 아래 감사 문서를 참고하세요.

- `/healthz`: 프로세스 상태. 성공해도 업스트림 검색 엔진이 동작한다는 보장은 아닙니다.
- `/config`: 등록된 엔진과 공개 설정 정보.
- `/stats`, `/metrics`: 런타임 통계와 메트릭.
- 엔진별 타임아웃, 일시 중지, 빈 결과 휴식으로 쓸 수 없는 출처를 처리합니다.
- `RUSTXNG_IDLE_TIMEOUT=300`: 요청 없이 5분이 지나면 종료합니다.
- 선택적 Valkey/Redis 캐시와 데이터베이스 기반 엔진으로 다른 배포 형태도 지원합니다.
- 실험적 로컬 인덱스 `rustxng-index`는 기본적으로 꺼져 있습니다.

[2026-11-02 감사](docs/AUDIT-2026-11-02.md)에는 SSRF 검증, 안전하지 않은 결과 URL 스킴,
응답 크기 제한, 캐시 격리 등 열린 문제가 기록되어 있습니다. 로컬 에이전트용으로는 loopback을 사용하고,
서비스를 외부에 공개하거나 신뢰할 수 없는 페이지 가져오기를 켜기 전에 이 내용을 검토하세요.

## 측정한 검색 성능

2026-11-02의 검색 전용 SealQA 측정은 기본 설정, 질문당 결과 10개,
LLM 없음, 로컬 인덱스 없음 조건으로 실행했습니다.

| 데이터셋 | 질문 수 | 참조 페이지 @1 | @5 | @10 | 응답 시간 중앙값 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Seal-0 | 111 | 47.75% | 54.95% | 65.77% | 1.85 s |
| Seal-Hard | 254 | 39.37% | 49.21% | 61.42% | 1.53 s |

이 수치는 참조 페이지 검색률이며 **답변 정확도가 아닙니다**. Seal-0은 Seal-Hard의 부분집합이고,
겹치는 질문은 같은 관측값을 씁니다. 결과는 네트워크와 업스트림 가용성에 따라 달라집니다.
[측정 절차와 질문별 결과](docs/SEALQA.md)를 참고하세요.
이전 측정과 상용 제공자 비교는 각자의 데이터셋과 조건과 함께 [BENCHMARK.md](docs/BENCHMARK.md)에 있습니다.

## 제공자 연동

엔진 인터페이스는 요청 헤더, 폼 데이터, JSON을 통해 인증이 필요한 HTTP API를 지원합니다.
선택적 제공자는 각자의 자격 증명이 필요하고 사용량에 따라 요금이 부과될 수 있습니다.
HTTP API에서는 `max_results`, 도메인 필터, 클라이언트 쪽 필드 추리기를 사용하세요.
MCP는 관련 단락과 직렬화 기준 문자·바이트 한도도 제공합니다. 특정 토크나이저 기준의
엄격한 예산은 향후 과제입니다.

빈 결과를 계속 반환하는 엔진은 자동으로 휴식 상태가 됩니다. `search.empty_result_limit`(기본 8, 0이면 끔)와 `search.empty_result_cooldown`(기본 300초)으로 설정합니다. `/stats`(HTML의 Resting 열), `/stats/errors`(JSON의 `resting` / `resting_for` 필드), `/metrics`(`searxng_engines_resting`, `searxng_engines_resting_seconds` 게이지)에서 확인할 수 있습니다.

## 문서

| 문서 | 내용 |
| --- | --- |
| [에이전트 설치](docs/AGENT-SETUP.md) | 설치, 서비스 재사용, 검색 확인, 접속 주소 보고 |
| [AI 인프라](docs/AI-INFRA.md) | 설계 동기. 현재 제약은 감사 문서를 참고하세요 |
| [SealQA](docs/SEALQA.md) | 검색 벤치마크와 재현 가능한 결과 |
| [벤치마크](docs/BENCHMARK.md) | 이전 성능·품질 측정 |
| [호환성](docs/COMPATIBILITY.md) | SearXNG 호환성과 차이점 |
| [지원 범위](docs/COVERAGE.md) | 엔진 지원 현황 |
| [로컬 인덱스](docs/LOCAL-INDEX.md) | 선택적 크롤링과 BM25 인덱스 |
| [rdsh](docs/RDSH.md) | rdsh 연동 |
| [감사](docs/AUDIT-2026-11-02.md) | 발견 사항, 재현 로그, 우선순위 |
| [보안](SECURITY.md) | 취약점 보고와 의존성 권고 |

링크된 문서는 영어 또는 일본어로 작성되어 있습니다.

## 커뮤니티와 유지보수

질문과 아이디어: [Discussions](https://github.com/sahenjp/RustXNG/discussions).
포크와 PR로 기여하거나 [영역 유지보수자로 신청](https://github.com/sahenjp/RustXNG/issues/new?template=maintainer_application.yml)하세요.
[거버넌스](GOVERNANCE.md), [유지보수자 명단](MAINTAINERS.md),
[FAQ](docs/FAQ.md), [로드맵](docs/ROADMAP.md)을 참고하세요. 기여하는 데 초대는 필요 없습니다.

## 기여하기

[CONTRIBUTING.md](CONTRIBUTING.md)를 참고하세요. 변경 사항은 다음으로 검증합니다.

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## 라이선스

[AGPL-3.0-or-later](LICENSE). RustXNG는 [SearXNG](https://github.com/searxng/searxng)를 바탕으로 합니다.
출처 표기는 [NOTICE](NOTICE)에 있습니다. 수정한 버전을 운영할 때의 소스 배포 의무는 라이선스를 참고하세요.

## 업데이트와 프록시 환경

`rustxng --check-update`로 최신 stable 공식 GitHub 릴리스를 확인하거나,
`rustxng --update`로 맞는 빌드 대상을 내려받아 SHA-256 파일을 검증하고
실행 파일을 교체합니다. 설정은 유지됩니다. 업데이트 후에는 서비스를 재시작하세요.
실행 파일이 있는 디렉터리에 쓰기 권한이 있어야 합니다. 컨테이너 이미지는
컨테이너 런타임으로 업데이트하고, 패키지 관리자로 설치했다면 원래 패키지 관리자를 사용하세요.
사용자 지정 대상이나 지원하지 않는 대상은 소스에서 다시 빌드하세요.
업데이트의 TLS 검증은 검색 설정과 관계없이 항상 켜져 있습니다.

검색과 업데이트 모두 OS의 CA 인증서를 신뢰합니다. 사용자 지정 PEM 번들은 `SSL_CERT_FILE`로,
검색용 추가 CA 인증서는 `outgoing.ca_bundle`로 지정하세요.
`outgoing.ca_bundle`은 공유 검색 HTTP 클라이언트에만 적용되며, 독립된
임베딩·LLM·크롤러 클라이언트에는 적용되지 않습니다. `outgoing.verify = true`를 유지하세요.

혼잡한 나가는 프록시 환경에서는 사용자 설정에 `search.strategy = "staged"`,
`search.staged_tier = 8`, `search.staged_min_results = 10`,
`search.timeout_limit = 15.0`, `outgoing.request_timeout = 10.0`을 넣으세요.
이 선택 설정은 엔진 요청을 나눠 보내고 마감 시간을 늘립니다.

Google 검색 엔진, 자동 완성, 외부 bang, YouTube, Google 번역 연동,
Material Icons, Google 기반 검색 프런트엔드는 제거되었습니다.
이전 사용자 설정에 해당 항목이 있으면 서버를 시작하기 전에 삭제하세요.
