# RustXNG

[English](README.md) · [日本語](README.ja.md)

RustXNG는 [SearXNG](https://github.com/searxng/searxng)를 바탕으로 만든 Rust 기반 셀프 호스팅 메타 검색 엔진입니다. 여러 외부 검색 제공자의 결과를 합치고, JSON API와 MCP를 통해 AI 에이전트가 검색 결과와 페이지 본문을 사용할 수 있게 합니다.

웹 전체를 자체적으로 크롤링하거나 대규모 검색 색인을 운영하지는 않습니다. `rustxng-index`로 선택한 사이트의 로컬 색인을 추가하거나 Wikipedia 공식 덤프를 별도로 색인할 수 있습니다.

## 빠른 시작

[Releases](https://github.com/sahenjp/RustXNG/releases)에서 운영체제에 맞는 바이너리를 받을 수 있습니다. 실행 전에 함께 배포된 SHA-256 체크섬을 확인하세요.

Cargo로 설치하려면 다음 명령을 실행합니다.

<!-- x-release-please-start-version -->
```bash
cargo install --git https://github.com/sahenjp/RustXNG.git --tag v0.29.0 --locked
```
<!-- x-release-please-end -->

소스 설치에는 Rust 1.88 이상과 C 빌드 도구가 필요합니다. 배포 바이너리 선택 및 검증 방법은 [에이전트 설치 안내](docs/AGENT-SETUP.md)를 참고하세요.

로컬 서버를 시작합니다.

```bash
rustxng --bind 127.0.0.1 --port 29483
```

다른 터미널에서 검색합니다.

```bash
curl --fail --get 'http://127.0.0.1:29483/search' \
  --data-urlencode 'q=tokio runtime documentation' \
  --data 'format=json' --data 'max_results=5'
```

결과에는 제목, URL, 발췌문, 검색 제공자 정보가 포함됩니다. `language=ja`, `engines=wikipedia`, `include_domains=docs.rs`, `exclude_domains=example.com`, `time_range=day`, `include_raw_content=1` 등의 매개변수를 사용할 수 있습니다. 기간 필터의 정확도는 제공자마다 다릅니다.

브라우저 화면은 <http://127.0.0.1:29483/>에 있습니다. JSON 외에 CSV, RSS, HTML 출력도 지원합니다.

## AI 에이전트에서 사용하기

내장 MCP 서버를 시작합니다.

```bash
rustxng mcp
```

MCP는 읽기 전용 도구 두 개를 제공합니다.

- `search`: 정렬된 검색 결과를 반환합니다. 기본 5개, 최대 10개이며 발췌문은 결과당 500자로 제한됩니다.
- `read_page`: 같은 세션에서 `search`가 반환한 URL의 본문을 읽습니다. `query`를 지정하면 추출된 페이지 전체에서 관련 문장 최대 3개를 선택합니다.

`read_page`는 기본 4,000자를 반환하고 `max_chars`로 최대 20,000자를 지정할 수 있습니다. `offset`과 `next_offset`으로 이어서 읽을 수 있으며, `truncated`와 `source_truncated`로 생략 여부를 확인할 수 있습니다. 응답의 문자 수와 바이트 수는 제한하지만 모델별 토큰 수를 정확하게 제한하지는 않습니다.

페이지를 읽을 때 대상 주소와 리디렉션을 검사해 비공개 IP 연결을 차단하고 robots.txt(RFC 9309)를 준수합니다. MCP에서는 임의의 URL을 바로 읽을 수 없습니다. 검색 결과와 본문은 지시문이 아닌 신뢰할 수 없는 외부 데이터로 취급해야 합니다.

HTTP API는 `include_raw_content=1`로 본문을 가져올 수 있습니다. PDF 텍스트 추출에는 Poppler(`pdftotext`)가 필요하며 스캔 PDF에는 별도 OCR이 필요합니다. JavaScript로 생성되는 본문을 읽으려면 선택 사항인 [Firefox 렌더러](tools/page-renderer/README.md)를 설정하세요.

## 검색 프로필

`--profile`로 검색 제공자 구성을 선택합니다. 기본값은 `standard`입니다.

| 프로필 | 명령 | 검색 원천 |
| --- | --- | --- |
| Standard | `rustxng` | 엔드포인트·사용 조건을 기록한 키 없는 항목 34개와 로컬 색인 |
| API | `rustxng --profile api` | Standard에 명시적으로 설정한 유료 Exa Search 추가 (`EXA_API_KEY`) |
| Clean | `rustxng --profile clean` | 선택된 공개 API와 로컬 색인 |
| Full | `rustxng --profile full` | 자동 접근 제한 또는 허가가 확인되지 않은 HTML 검색 제공자도 포함 |

**Full을 사용해도 제공자 약관을 우회할 수 없습니다.** 필요한 허가를 확인하지 못한 제공자는 활성화하지 마세요. 개인용 서버도 이용 약관을 따라야 합니다. Clean 프로필 역시 법적 적합성을 보증하지 않습니다.

MCP에서도 `rustxng mcp --profile clean`처럼 선택할 수 있습니다. 일부 API 제공자는 키나 요금을 요구합니다. RustXNG를 로컬에서 실행해도 검색어는 활성화한 외부 제공자로 전송됩니다.

Standard는 검토하지 않은 제공자를 모두 기본 비활성화합니다. 검토 범위는 기록된 검색 용도이며 콘텐츠 라이선스, 할당량과 배포 목적의 조건은 계속 적용됩니다. 키가 존재하는 것만으로 API를 활성화하지 않습니다. [현재 측정 및 설정](docs/ISSUE-RESOLUTION-2026-11-05.md)을 참고하세요.

## 설치와 설정

Docker로 실행할 수도 있습니다.

```bash
docker run --rm -p 127.0.0.1:29483:29483 \
  -e SEARXNG_PORT=29483 ghcr.io/sahenjp/rustxng:latest \
  --bind 0.0.0.0 --port 29483
```

기본 설정은 로컬 접속만 받습니다. 외부에 공개한다면 인증과 접근 제어를 별도로 설정하세요.

기본 설정은 바이너리에 포함되어 있습니다. 필요한 항목만 TOML 파일에 작성해 덮어쓸 수 있습니다.

```bash
rustxng --config settings.toml
```

사용자 설정 파일은 선택한 프로필을 덮어쓰므로 전체 기본 설정을 복사할 때는 주의하세요.

`rustxng --check-update`로 업데이트를 확인하고, 지원되는 바이너리 설치에서는 `rustxng --update`를 사용할 수 있습니다. 컨테이너나 패키지 관리자로 설치했다면 원래의 업데이트 방법을 사용하세요.

`/healthz`는 프로세스 상태만 확인합니다. 실제 검색 가능 여부는 JSON 검색으로 검증하세요. 실행 통계는 `/stats`와 `/metrics`에서 볼 수 있습니다.

## 검색 품질과 제한

[2026년 11월 4일 비교](docs/COMPARISON-2026-11-04.md)는 기술 문서 100개 질문과 SealQA 254개 질문으로 세 프로필을 측정했습니다. 기술 문서의 Hit@10은 Standard 88, Clean 61, Full 98이었습니다. 측정 환경에는 로컬 색인이 포함되었습니다.

이는 정답으로 지정한 페이지를 검색 결과에서 찾은 비율이지 AI 답변의 정확도가 아닙니다. 평가 질문을 개발 과정에서도 사용했으며, 로컬 색인에는 평가에 관련된 문서 사이트가 포함되어 있습니다. 다른 검색 API와의 저장된 비교 결과 역시 측정 시점이 다릅니다. 이 수치만으로 일반적인 우열을 판단할 수 없습니다.

## 문서

- [에이전트 설치와 확인](docs/AGENT-SETUP.md)
- [제공자 약관과 설정](docs/PROVIDERS.md)
- [로컬 크롤러와 색인](docs/LOCAL-INDEX.md) · [Wikipedia 색인](docs/WIKIPEDIA-INDEX.md)
- [SearXNG 호환성](docs/COMPATIBILITY.md) · [과거 벤치마크](docs/BENCHMARK.md)
- [보안 정책](SECURITY.md) · [기여 안내](CONTRIBUTING.md) · [릴리스 절차](docs/RELEASING.md)

## 기여와 라이선스

버그 제보와 제안은 [Issues](https://github.com/sahenjp/RustXNG/issues), [Discussions](https://github.com/sahenjp/RustXNG/discussions)에서 받습니다. 풀 리퀘스트도 환영합니다. 자세한 내용은 [CONTRIBUTING.md](CONTRIBUTING.md), [GOVERNANCE.md](GOVERNANCE.md), [MAINTAINERS.md](MAINTAINERS.md)를 참고하세요.

RustXNG는 [AGPL-3.0-or-later](LICENSE) 라이선스로 제공됩니다. SearXNG와 외부 기여자에 대한 출처 표시는 [NOTICE](NOTICE)에 기록되어 있습니다. 코드 라이선스가 외부 검색 제공자의 데이터를 수집하거나 재배포할 권한까지 부여하는 것은 아닙니다.
