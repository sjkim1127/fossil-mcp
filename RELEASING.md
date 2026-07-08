# Releasing fossil-mcp

## 릴리즈 절차 (태그 기반 자동화)

fossil-mcp는 태그를 push하면 GitHub Actions가 5개 플랫폼 바이너리를 자동으로 빌드하고 GitHub Release를 생성합니다.

### 1. 릴리즈 전 체크리스트

```bash
# 1) 모든 테스트 통과 확인
cargo test --workspace

# 2) 경고 없는지 확인
cargo clippy --workspace --all-targets -- -D warnings

# 3) 포맷 확인
cargo fmt --all -- --check

# 4) CHANGELOG.md 업데이트
#    [Unreleased] → [X.Y.Z] - YYYY-MM-DD 로 변경
#    새 [Unreleased] 섹션 추가

# 5) Cargo.toml의 workspace.package.version 업데이트
#    version = "X.Y.Z"
```

### 2. 태그 push (릴리즈 트리거)

```bash
# 버전 태그 push → Release 워크플로우 자동 실행
git tag v0.2.0 -m "release: v0.2.0"
git push origin v0.2.0
```

- `v1.0.0` → 정식 릴리즈
- `v1.0.0-rc.1` → pre-release (GitHub에서 Pre-release 표시)
- `v1.0.0-beta.1` → pre-release

### 3. 생성되는 아티팩트

| 파일 | 플랫폼 |
|---|---|
| `fossil-mcp-vX.Y.Z-x86_64-unknown-linux-musl.tar.gz` | Linux x86\_64 (정적 링킹) |
| `fossil-mcp-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz` | Linux ARM64 |
| `fossil-mcp-vX.Y.Z-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `fossil-mcp-vX.Y.Z-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon |
| `fossil-mcp-vX.Y.Z-x86_64-pc-windows-msvc.zip` | Windows x86\_64 |
| `SHA256SUMS.txt` | 체크섬 파일 |

### 4. 릴리즈 노트 자동화

`CHANGELOG.md`에 해당 버전 섹션(`## [X.Y.Z]`)이 있으면 그 내용이 릴리즈 노트로 자동 사용됩니다.

### 5. 핫픽스 릴리즈

```bash
# main 브랜치에서 패치 수정 후:
git tag v0.1.1 -m "fix: 크리티컬 버그 수정"
git push origin v0.1.1
```

## CI 동작 방식

| 이벤트 | 실행 워크플로우 |
|---|---|
| PR → main | CI (fmt + clippy + test × 2 OS + MSRV) |
| push → main | CI |
| tag `v*` push | Release (5개 플랫폼 빌드 + GitHub Release 생성) |

## 로컬 릴리즈 빌드 (검증용)

```bash
# macOS Apple Silicon 기준
cargo build --release --locked --bin fossil-mcp

# Linux musl (Docker 또는 musl-cross 설치 후)
cargo build --release --locked --bin fossil-mcp --target x86_64-unknown-linux-musl
```
