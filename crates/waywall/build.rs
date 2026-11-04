//! PurismCore(Cubism Core 호환 오픈소스 재구현, MIT)를 컴파일해 정적 링크한다 (docs/development.md#live2d-purismcore-m9).
//! 소스 위치는 flake가 `PURISM_CORE_SRC`로 넘긴다(devShell과 패키지 빌드 모두).

use std::path::PathBuf;

/// PurismCore Makefile의 `SRC` (v1.1.0).
const SOURCES: [&str; 17] = [
    "core.c",
    "debug.c",
    "arena.c",
    "math2.c",
    "moc3.c",
    "verify.c",
    "model.c",
    "update.c",
    "param.c",
    "part.c",
    "deformer.c",
    "artmesh.c",
    "glue.c",
    "offscreen.c",
    "blendshape.c",
    "interpolate.c",
    "render.c",
];
const SRC_ENV: &str = "PURISM_CORE_SRC";

fn main() {
    println!("cargo:rerun-if-env-changed={SRC_ENV}");
    let root = PathBuf::from(std::env::var_os(SRC_ENV).unwrap_or_else(|| {
        panic!("{SRC_ENV} is not set: build inside `nix develop` or with `nix build`")
    }));
    let src = root.join("src");
    for f in SOURCES {
        println!("cargo:rerun-if-changed={}", src.join(f).display());
    }
    cc::Build::new()
        .files(SOURCES.iter().map(|f| src.join(f)))
        .include(root.join("include"))
        .include(&src)
        // ABI v6 (Purism 기본값, Cubism Core 6.0).
        .define("PSM_GIT_HASH", "\"v1.1.0\"")
        .warnings(false)
        .compile("PurismCore");
    println!("cargo:rustc-link-lib=m");
}
