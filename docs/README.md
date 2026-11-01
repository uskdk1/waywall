# waywall 문서

waywall은 Wayland 컴포지터용 라이브 배경화면 데몬입니다. Windows의 Wallpaper Engine처럼
동영상, 웹(HTML/JS), GLSL 셰이더, 이미지/GIF, 그리고 빛·파티클·블룸을 조합하는 씬 배경을
바탕화면에 띄우고, 마우스 움직임과 클릭에 반응하는 배경을 지원합니다.

## 목표

- 5종 배경: 동영상 · 웹 · 셰이더 · 이미지/GIF · 씬(빛, 파티클, 후처리)
- 상시 실행 비용 최소화 — GTK 없는 네이티브 데몬, Vulkan(wgpu), 영상 zero-copy
- 마우스 상호작용 (움직임, 클릭)
- 창이 배경을 가리면 자동 일시정지
- 모니터별 개별 배경
- 오디오 재생과 볼륨/음소거 제어
- **Wayland 표준 프로토콜 우선** — 표준이 없을 때만 확장/컴포지터 전용 기능 사용
- **Nix만으로 재현 가능한 개발·빌드 환경**
- **설정 주도** — 코드에는 색·모양·수치를 두지 않고, 모든 것을 config와 assets로 커스텀

1차 범위는 데몬 + CLI이며, GUI 매니저(썸네일 라이브러리)는 이후 단계입니다.

## 문서 목차

| 문서 | 내용 |
|---|---|
| [architecture.md](architecture.md) | 전체 구조, 컴포넌트 책임, 데이터 흐름 |
| [protocols.md](protocols.md) | 사용하는 Wayland 프로토콜과 선택 근거, 폴백 체계 |
| [wallpaper-format.md](wallpaper-format.md) | 배경 타입, `project.toml`·`scene.toml` 스펙, 셰이더/웹 배경 작성법 |
| [config.md](config.md) | `config.toml` 스키마와 예시 |
| [cli-ipc.md](cli-ipc.md) | CLI 명령, IPC 소켓 메시지, 웹 헬퍼 프로토콜 |
| [development.md](development.md) | Nix 개발 환경, 빌드·실행·테스트 방법 |
| [roadmap.md](roadmap.md) | 단계별 마일스톤 |
