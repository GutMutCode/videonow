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
apps/videonow-desktop       VideoNow 앱 창: 편집·모션·이미지 작업 공간, 자산 목록, 명령 팔레트, 에이전트 패널
apps/videonow-cli           videonow 실행 파일: demo, run, commands, mcp, mcp --attach
crates/videonow-core        하나의 프로젝트: 세 엔진 세션, 자산 표, 엔진 사이의 연결, 명령 진입점
crates/videonow-automation  MCP 서버 (command_list, command_run, command_batch, doc_inspect, render_preview)
crates/videonow-eval        에이전트 평가: 시나리오 8개와 지표, 실제 에이전트 세션 채점
engines/{film,effect,photo} 가져온 엔진 (각자의 crates/, assets/, docs/)
```

- 컴포지션과 이미지 문서는 타임라인 위의 **중첩 클립**입니다. 각 엔진이 자기 부분을 렌더하고, 편집은 다음 프레임에 그대로 보입니다.
- 모든 기능은 `도메인:명령` 하나로 호출합니다: `timeline:…`, `motion:…`, `image:…`, `studio:…`.
- 모든 명령은 결과와 함께 **실행 영수증**을 돌려줍니다: 무엇이 바뀌었는지, 리비전, 확인한 것과 확인하지 않은 것.

설계 전체는 설계 문서(VideoNow 설계)에 있습니다.

## 빌드와 실행

Rust 1.95 이상. NixOS에서는 `nix run github:GutMutCode/videonow` (또는 `nix develop`).

```sh
cargo build --release -p videonow-desktop -p videonow-cli

# 앱 (데모 프로젝트로 시작)
./target/release/videonow-desktop --demo

# 이미지 → 컴포지션 → 타임라인을 거치는 데모 프로젝트를 만들고 프레임을 렌더
./target/release/videonow demo out/

# 명령 목록 (세 엔진 + 스튜디오 명령, 효과 분류 포함)
./target/release/videonow commands blur

# JSON Lines 스크립트 실행
./target/release/videonow run script.jsonl

# AI 에이전트용 MCP 서버 (헤드리스, 자기 프로젝트)
claude mcp add videonow -- /abs/path/target/release/videonow mcp

# 열려 있는 앱의 프로젝트를 에이전트가 함께 편집 (앱의 Agent 패널에서 허용을 켠 뒤)
claude mcp add videonow-app -- /abs/path/target/release/videonow mcp --attach
```

앱에서는:

- 위쪽의 **Edit / Motion / Image**가 세 편집기입니다. 컴포지션과 이미지는 한 프로젝트의 자산이고, 타임라인이나 컴포지션에 놓아도 계속 살아 있습니다. 타임라인에서 중첩 클립을 고르면 "Edit … in Motion ›"으로 바로 그 컴포지션을 엽니다.
- **Commands**(Ctrl+Shift+P, F1)는 세 엔진과 스튜디오의 명령 2,400여 개를 찾아 실행하고, 결과와 영수증을 보여 줍니다. 실행 전에 Dry run으로 무엇이 바뀔지 볼 수 있습니다.
- **Agent** 패널에서 에이전트 접근을 켜고, 무엇을 허용할지(파일 쓰기, 삭제·교체, 외부 접근, 프로젝트 폴더 밖) 정합니다. 사람과 에이전트의 모든 변경이 영수증과 함께 활동 기록에 쌓입니다.

스크립트 예:

```jsonl
{"id": "studio:comp.new", "params": {"name": "Title", "duration": 4}}
{"id": "motion:layer.newText", "params": {"asset": 1, "text": "Hello", "size": 160}}
{"id": "timeline:file.newSequence", "params": {"name": "Main"}}
{"id": "studio:asset.place", "params": {"asset": 1, "track": "V1"}}
{"id": "studio:render.frame", "params": {"seconds": 1, "path": "frame.png"}}
```

## 에이전트를 위한 약속

- 모든 명령은 `domain:command` 하나의 표면이고, 효과 분류(read, edit, destructive, fileWrite, external)가 붙어 있습니다.
- 모르는 매개변수는 거부하고 받는 키를 알려 줍니다. 틀린 명령 이름에는 가장 가까운 명령을 제안합니다.
- `request_id`로 재시도해도 한 번만 실행됩니다. `expect_revision`이 맞지 않으면(사람이나 다른 에이전트가 그 사이 바꿨으면) 무엇이 바뀌었는지와 함께 거부합니다.
- `command_batch`는 기본적으로 원자적입니다: 한 단계가 실패하면 앞 단계를 되돌립니다. `"$0.asset"`처럼 앞 단계 결과를 씁니다.
- `dry_run`, `render_preview`의 `steps`, 원자적 일괄 작업은 복구 가능한 문서 명령만 받습니다. 파일 저장·내보내기, 외부 실행, 프로젝트 교체, 실행 취소·히스토리 변경, 내부 명령을 실행하는 스크립트·배치 등은 어떤 단계도 시작하기 전에 거부합니다. 저장은 편집 결과를 확인한 뒤 별도 명령으로 실행하세요. `atomic:false` 일괄 작업은 앞 단계의 결과를 되돌리지 않습니다.
- 파일 권한은 저장 위치·기본 출력 경로까지 해석한 뒤 검사합니다. `motion:prop.set`의 `path` 같은 편집 속성 경로는 파일 경로와 구분합니다.
- `videonow mcp --log FILE`은 모든 도구 호출을 남기고, `videonow-eval score FILE PROJECT`가 그 세션을 채점합니다.

## 상태

설계 문서의 로드맵 1–4단계를 구현했습니다. 단계별 상태와 남은 일은 설계 문서와 `docs/status.md`에 있습니다.

## 라이선스

MIT 또는 Apache-2.0 중 선택. 가져온 엔진의 저작권과 라이선스는 [NOTICE](NOTICE)와 각 `engines/*` 디렉터리에 그대로 있습니다. ArtCraft 이름과 로고는 상표이며 VideoNow에서 쓰지 않습니다.
