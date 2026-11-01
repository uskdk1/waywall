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
- `set`의 상대 경로는 CLI가 절대 경로로 바꿔서 보냄 (데몬의 작업 디렉터리와 무관하게)
- `pause`는 `User` 사유를 추가, `resume`은 `User` 사유만 제거 — 가림 때문에 멈춘 배경은
  `resume`해도 가림이 풀릴 때까지 멈춰 있음
- 데몬이 없으면 오류 메시지와 함께 종료 코드 `2`

### `waywall list` 출력 예

```
DP-1   2560x1440@1     rain.mp4   video  playing  (dmabuf)
eDP-1  1920x1200@1.25  forest/    scene  paused   (hidden:frame)
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

- `state`: `playing` | `paused` | `idle`(배경 없음) | `error`
- `reason`: `user` | `hidden` | `covered`
- `source`: `cli` | `frame` | `foreign_toplevel` | `niri`
- `render_path`: `shm` | `dmabuf` | `shm-fallback` | `gpu` | `web` — 성능 문제 진단용
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
