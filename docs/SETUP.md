# Сетап на новой машине

Как поставить всё, что нужно для разработки, на чистый Mac (Apple Silicon) и проверить, что
проект собирается. Как играть: [`PLAYING.md`](PLAYING.md). Правила для агентов:
[`AGENTS.md`](../AGENTS.md).

## 1. Инструменты

```sh
xcode-select --install                      # компилятор C, линкер, git
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
brew install gh                             # GitHub CLI: клон приватного репо, PR
gh auth login
```

**Rust** ставится через rustup. Нужную версию (1.99.0, `rust-toolchain.toml`) вместе с
`rustfmt` и `clippy` cargo подтянет сам при первой команде в каталоге проекта.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # по умолчанию, в ~
source "$HOME/.cargo/env"                   # rustup пишет это и в ~/.zshenv
```

**Без этого не пройдёт gate:**

```sh
cargo install cargo-nextest --locked        # тест-раннер gate.sh (на cargo test он не откатывается)
```

**Для веб-сборки** (`scripts/build-web.sh`): версия wasm-bindgen-cli должна совпадать с
`wasm-bindgen` в `Cargo.lock`. Если нет, скрипт скажет, какую ставить.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
```

**Node** нужен только чтобы перерисовать иконки интерфейса или импортировать модели
(`tools/assets`). Для сборки игры он не нужен.

```sh
brew install node                           # или nvm; у нас было v24
cd tools/assets && npm install              # затем `node icons.mjs` перерисует assets/ui/icons
```

## 2. Код

```sh
gh repo clone vladborsh/cityplanner ~/Documents/cityplanner
cd ~/Documents/cityplanner
```

Основная ветка `master`. GitHub Actions недоступны (оплата), поэтому проверка зелёного только
локальная (шаг 4).

## 3. Кэш компиляции (sccache) — не обязателен

Нужен, только если параллельно работают несколько агентов в своих worktree, у каждого свой
`CARGO_TARGET_DIR` (таблица слотов в `AGENTS.md`). Тогда собранные зависимости (Bevy и др.)
переиспользуются между слотами. Без кэша холодная сборка в каждом слоте занимает около 10 минут.
С одной рабочей копией он почти ничего не даёт: cargo и так держит сборку в `target/`.

```sh
brew install sccache
mkdir -p ~/.cargo && cat >> ~/.cargo/config.toml <<'EOF'
[build]
rustc-wrapper = "/opt/homebrew/bin/sccache"
EOF
```

Чтобы слоты попадали друг в друга, перечислите каталог target **каждого** слота в `basedirs`
в `~/Library/Application Support/Mozilla.sccache/config`. Без этого путь входит в ключ кэша, и
другой слот промахивается в 100 % случаев.

```toml
basedirs = ["/Users/<you>/Documents/cityplanner/target",
            "/Users/<you>/orca/workspaces/cityplanner/.cargo-target/dev-1"]  # и т. д.
[cache.disk]
size = 53687091200   # 50 GiB
```

После правки: `sccache --stop-server; sccache --start-server`. Проверить: `sccache --show-stats`.

## 4. Проверка

```sh
scripts/gate.sh                             # должен закончиться строкой «PASS: N tests», код 0
cargo build --release -p cp_desktop         # игра: target/release/cp_desktop
scripts/build-web.sh                        # веб: dist/web (опционально)
```

Первый `gate.sh` долгий: Bevy собирается целиком (холодная сборка всего workspace на M4 Pro
около 10 минут). Что входит в gate и как вливать ветки: `AGENTS.md`, раздел «Build and test».

Фикстура `fixtures/saves/bench-100k.sav` в git не лежит (большая). Она нужна только бенчу
на 100k жителей и пересоздаётся генератором бенча (release, около 10 минут).

## 5. Агенты (по желанию)

- **Слоты.** Worktree для агентов (dev-1…, tester и т. д.) создаются из веток заново:
  `git worktree add ../<slot> <ветка>`. У каждого слота свой `CARGO_TARGET_DIR` и
  `CITYPLANNER_DATA_DIR` (таблица в `AGENTS.md`). Каталоги `.data/` и `target/` в git не
  входят и не переносятся.
- **Память Claude Code** лежит вне репозитория:
  `~/.claude/projects/<путь проекта с «-» вместо «/»>/memory/`. Это рабочие заметки
  координатора: слоты, хэндлы терминалов, история решений. В ней есть пути и состояние этой
  машины, поэтому в git её не кладём. Правила, которые должен знать любой агент, живут в
  `AGENTS.md`. Если заметки нужны на новой машине, скопируйте каталог целиком. При другом
  пути проекта переименуйте его под новый путь.
