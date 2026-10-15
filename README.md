# VideoNow

하나의 영상 제작 소프트웨어. 타임라인 편집, 모션그래픽과 합성, 이미지 편집을 한 프로젝트 안에서 합니다.

VideoNow는 세 오픈소스 앱의 엔진을 하나로 묶어 시작했습니다.

| 영역 | 엔진 | 가져온 곳 |
|---|---|---|
| 타임라인: 편집, 오디오, 자막, 코덱, 내보내기 | `engines/film` | [FilmCraft](https://github.com/storytold/filmcraft) |
| 모션: 컴포지션, 키프레임, 이펙트, 트래킹 | `engines/effect` | [EffectCraft](https://github.com/storytold/effectcraft) |
| 이미지: 레이어 문서, 브러시, 보정, 리터치 | `engines/photo` | [PhotoCraft](https://github.com/storytold/photocraft) |

세 엔진은 한 번 복사한 뒤 이 레포에서 독립적으로 발전시킵니다(원본 커밋은 `engines/UPSTREAM.txt`). Adobe 앱과의 1:1 동등성이 아니라, 영상 한 편을 끝까지 만드는 데 필요한 핵심 기능이 목표입니다.

## 구조

```text
crates/videonow-core        하나의 프로젝트: 세 엔진 세션, 자산 표, 엔진 사이의 연결, 명령 진입점
crates/videonow-automation  MCP 서버 (command_list, command_run, command_batch, doc_inspect, render_preview)
apps/videonow-cli           videonow 실행 파일: demo, run, commands, mcp
engines/{film,effect,photo} 가져온 엔진 (각자의 crates/, assets/, docs/)
```

- 컴포지션과 이미지 문서는 타임라인 위의 **중첩 클립**입니다. 각 엔진이 자기 부분을 렌더하고, 편집은 다음 프레임에 그대로 보입니다.
- 모든 기능은 `도메인:명령` 하나로 호출합니다: `timeline:…`, `motion:…`, `image:…`, `studio:…`.
- 모든 명령은 결과와 함께 **실행 영수증**을 돌려줍니다: 무엇이 바뀌었는지, 리비전, 확인한 것과 확인하지 않은 것.

설계 전체는 설계 문서(VideoNow 설계)에 있습니다.

## 빌드와 실행

Rust 1.95 이상.

```sh
cargo build --release -p videonow-cli

# 이미지 → 컴포지션 → 타임라인을 거치는 데모 프로젝트를 만들고 프레임을 렌더
./target/release/videonow demo out/

# 명령 목록 (세 엔진 + 스튜디오 명령, 효과 분류 포함)
./target/release/videonow commands blur

# JSON Lines 스크립트 실행
./target/release/videonow run script.jsonl

# AI 에이전트용 MCP 서버
claude mcp add videonow -- /abs/path/target/release/videonow mcp
```

스크립트 예:

```jsonl
{"id": "studio:comp.new", "params": {"name": "Title", "duration": 4}}
{"id": "motion:layer.newText", "params": {"asset": 1, "text": "Hello", "size": 160}}
{"id": "timeline:file.newSequence", "params": {"name": "Main"}}
{"id": "studio:asset.place", "params": {"asset": 1, "track": "V1"}}
{"id": "studio:render.frame", "params": {"seconds": 1, "path": "frame.png"}}
```

## 상태

0단계(통합 뼈대)입니다. 헤드리스 엔진, 중첩 렌더, 통합 명령, MCP가 동작합니다. 프로젝트 저장과 열기, 엔진 명령의 엄격한 검증은 1단계, 통합 UI는 2단계입니다.

## 라이선스

MIT 또는 Apache-2.0 중 선택. 가져온 엔진의 저작권과 라이선스는 [NOTICE](NOTICE)와 각 `engines/*` 디렉터리에 그대로 있습니다. ArtCraft 이름과 로고는 상표이며 VideoNow에서 쓰지 않습니다.
