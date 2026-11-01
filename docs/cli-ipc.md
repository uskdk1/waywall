# CLI와 IPC

## CLI

```
waywall daemon [--config <PATH>]
waywall set    [-o <OUTPUT>] [--save] <PATH|URL>
waywall pause  [-o <OUTPUT>]
waywall resume [-o <OUTPUT>]
waywall volume [-o <OUTPUT>] [--save] (<0-100> | --mute | --unmute)
waywall list   [--json]
waywall config defaults           # 내장 defaults.toml 출력 (데몬 불필요)
waywall config show [-o <OUTPUT>] # 모든 계층을 합친 최종 설정
waywall reload
waywall quit
```

- `-o` 생략 시 모든 출력에 적용
- `set`의 상대 경로는 CLI가 절대 경로로 바꿔서 보냄 (데몬의 작업 디렉터리와 무관하게).
  현재 디렉터리 기준으로 존재하지 않는 값(URL 등)은 그대로 보냄
- `pause`는 `User` 사유를 추가, `resume`은 `User` 사유만 제거 — 가림 때문에 멈춘 배경은
  `resume`해도 가림이 풀릴 때까지 멈춰 있음
- `volume`은 숫자와 `--mute`/`--unmute`를 함께 줄 수 있음 (둘 중 하나는 필수)
- 종료 코드: `0` 성공, `1` 데몬이 요청을 거절함(오류 메시지 출력), `2` 명령줄 사용법 오류, `3` 데몬이 실행 중이 아님

### 런타임 덮어쓰기

`set` · `volume` · `pause`는 설정 파일 위의 "CLI 계층"에 값을 둡니다([config.md](config.md#계층과-우선순위)).

- 값은 **출력 이름 기준**으로 기억합니다. 모니터를 뽑았다 다시 꽂아도 데몬이 살아 있는 동안은
  CLI로 바꾼 배경·볼륨·일시정지 상태가 그대로 복원됩니다.
- `-o` 없이 실행하면 "모든 출력" 값이 되고, 그 키에 대해 출력별로 지정했던 CLI 값은 지워집니다.
  (`set -o DP-1 a.png` 후 `set b.png` → 모든 출력이 `b.png`) 나중에 연결되는 출력에도 적용됩니다.
- `-o`로 지정한 출력이 현재 연결되어 있지 않으면 오류입니다.
- `reload`는 배경·볼륨·음소거 CLI 값을 지우고 설정 파일대로 되돌립니다. `pause` 상태는 설정이 아니라
  상태이므로 `reload` 후에도 유지됩니다.

### `--save`

데몬이 읽은 설정 파일(`daemon --config` 또는 기본 위치)을 `toml_edit`로 고칩니다. 주석과 서식은 보존하고,
파일이 없으면 만듭니다.

| 명령 | 기록 위치 |
|---|---|
| `set -o X --save P` | `[output."X"] wallpaper = P` |
| `set --save P` | `[default] wallpaper = P`, 그리고 모든 `[output.*]`의 `wallpaper` 삭제 (모든 출력이 P가 되도록) |
| `volume -o X --save …` | `[output."X"]`의 `volume` / `mute` |
| `volume --save …` | 최상위 `volume` / `mute`, 그리고 `[default]`·`[output.*]`의 같은 키 삭제 |

저장해도 CLI 계층 값은 그대로 두므로 화면은 바로 바뀌고, 다음 `reload` 이후에는 파일 값으로 같은 결과가 납니다.

### `set`의 응답 시점

`set`은 대상 출력에서 **새 배경이 실제로 표시된 뒤** 응답합니다.

1. 먼저 모든 대상 출력에 대해 경로 해석·타입 판별·옵션 검사를 합니다. 하나라도 실패하면 아무것도 바꾸지 않고
   `ok:false`를 돌려줍니다(이전 배경 유지).
2. 통과하면 CLI 계층에 기록하고 로딩을 시작합니다. 디코딩처럼 나중에 실패하는 경우는 `appearance.error` 규칙대로
   대체 화면을 띄우고, 응답은 `ok:false`와 출력별 이유입니다.

### `reload`

설정 파일을 다시 읽습니다. 파일에 오류가 있으면 `ok:false`를 돌려주고 기존 설정을 유지합니다.

- 출력별 최종 설정이나 전역 설정(`[types.*]`, `[detect]` 등)이 바뀐 출력만 배경을 다시 읽습니다.
- `[surface]`가 바뀌면 모든 layer surface를 다시 만듭니다.
- `[log]`, `[cursor].theme`/`size`는 데몬을 다시 시작해야 반영됩니다.
- 응답은 다시 읽기를 시작한 즉시 돌려줍니다. 배경 로딩 결과는 `list`로 확인합니다.

### `waywall list` 출력 예

크기는 물리 픽셀, `@` 뒤는 스케일입니다.

```
DP-1      2560x1440@1     rain.mp4  video  playing  vol 30        dmabuf
eDP-1     1920x1200@1.25  forest    scene  paused   vol 50 muted  gpu    (hidden:frame, user:cli)
HDMI-A-1  1920x1080@1     -         -      idle     vol 50        solid
```

## IPC

- 소켓: `$XDG_RUNTIME_DIR/waywall.sock` (`WAYWALL_SOCKET` 환경변수로 변경 가능)
- 권한: `0600`
- 형식: **JSON-lines** — 요청 한 줄, 응답 한 줄. 한 연결에서 여러 요청 가능
- 데몬 시작 시 소켓이 이미 있고 접속되면 "이미 실행 중"으로 종료, 접속 안 되면 낡은 소켓으로 보고 삭제

### 요청

모든 요청은 `cmd` 필드를 가집니다. `output`이 없거나 `null`이면 모든 출력.

```json
{"cmd":"set","output":"DP-1","path":"~/Wallpapers/rain.mp4","save":false}
{"cmd":"pause","output":null}
{"cmd":"resume","output":"eDP-1"}
{"cmd":"volume","output":null,"volume":30,"mute":null,"save":false}
{"cmd":"list"}
{"cmd":"reload"}
{"cmd":"quit"}
```

`volume`: `volume`(0–100)과 `mute`(bool) 중 바꿀 것만 지정, 나머지는 `null`.

### 응답

```json
{"ok":true,"data":null}
{"ok":false,"error":"unknown output: HDMI-A-9"}
```

`list` 응답의 `data`:

```json
{
  "outputs": [
    {
      "name": "DP-1",
      "description": "Dell Inc. DELL U2720Q",
      "width": 2560, "height": 1440, "scale": 1.0,
      "wallpaper": { "path": "~/Wallpapers/rain.mp4", "kind": "video", "name": "rain.mp4" },
      "render_path": "dmabuf",
      "volume": 30, "mute": false,
      "state": "paused",
      "pause_reasons": [ { "reason": "covered", "source": "foreign_toplevel" } ]
    }
  ]
}
```

- `state`: `playing` | `paused` | `loading` | `idle`(배경 없음) | `error`
- `reason`: `user` | `hidden` | `covered`
- `source`: `cli` | `frame` | `foreign_toplevel` | `niri`
- `render_path`: `solid`(단색) | `shm` | `dmabuf` | `shm-fallback` | `gpu` | `web` — 성능 문제 진단용.
  아직 아무것도 표시하지 않았으면 `null`
- `wallpaper`: 설정된 배경(로딩 중·실패 포함). 없으면 `null`. `width`/`height`는 물리 픽셀
- 정적 이미지는 멈출 것이 없지만 `pause` 상태는 그대로 기록·표시합니다
- `state = "error"`일 때 `error` 필드에 메시지 (예: 셰이더 컴파일 오류)

### 오류 처리
- 잘못된 JSON / 알 수 없는 `cmd` → `{"ok":false,"error":...}` 응답 후 연결 유지
- 배경 로딩 실패(파일 없음, 셰이더 컴파일 실패) → `set`은 `ok:false`와 이유 반환, 이전 배경 유지

## 웹 헬퍼 프로토콜

데몬 ↔ `waywall-web` 간 내부 프로토콜입니다(사용자가 직접 쓰지 않음). 헬퍼의 stdin(데몬→헬퍼),
stdout(헬퍼→데몬)으로 JSON-lines를 주고받고, 로그는 stderr로 냅니다. 타입은 `waywall-proto`에 정의합니다.

데몬 → 헬퍼:
```json
{"op":"show","output":"DP-1","url":"file:///home/me/Wallpapers/stars/index.html",
 "properties":{"density":1.0},"volume":0.5,"muted":false,"interactive":true}
{"op":"hide","output":"DP-1"}
{"op":"pause","output":"DP-1"}
{"op":"resume","output":"DP-1"}
{"op":"volume","output":"DP-1","volume":0.3,"muted":false}
{"op":"properties","output":"DP-1","properties":{"density":2.0}}
{"op":"quit"}
```

헬퍼 → 데몬:
```json
{"ev":"ready"}
{"ev":"shown","output":"DP-1"}
{"ev":"visibility","output":"DP-1","hidden":true}
{"ev":"error","output":"DP-1","message":"load failed: …"}
```

- 헬퍼는 `pause`/`resume`을 frame 기아 판정과 무관하게 데몬 지시대로 따르고, frame 기아는 `visibility`로 보고만 합니다.
  pause 여부의 최종 결정은 항상 데몬이 합니다(사유 합성은 한 곳에서).
- stdin이 닫히면(데몬 종료) 헬퍼도 종료합니다.

## 로드맵: 이벤트 구독
GUI 매니저용으로 `{"cmd":"subscribe"}` 이후 상태 변경을 계속 흘려보내는 스트림을 추가할 예정.
