# CLAUDE.md — s-dock project context

## Что это за проект

s-dock — док-бар для Hyprland (только Hyprland), спутник ashell-ru.
Пишется в том же стеке и стиле, что `~/projects/ashell-ru`:
Rust + iced_layershell fork v0.1.5, Elm-архитектура, TOML-конфиг с hot-reload.

**ВАЖНО: контекст сессии может обнуляться. Порядок действий при старте:**

1. Прочитай `docs/SPEC.md` — замороженная утверждённая спека и все решения владельца.
2. Прочитай `docs/PLAN.md` — план этапов с чекбоксами; первый `- [ ]` = текущая задача.
3. Технические паттерны (bootstrap layer-shell, multi-output, Hyprland IPC) —
   в конце docs/PLAN.md, источник образцов: `~/projects/ashell-ru/src/`.
4. Не менять решения из SPEC.md без явной просьбы владельца.

## Рабочие соглашения

- Ответы владельцу — на русском.
- Качество вместо тестов: `make check` (cargo fmt --check + clippy -D warnings),
  ноль предупреждений, перед каждым коммитом.
- Коммиты: `feat(scope): …`, `fix(scope): …`, `chore: …`; пушить только по просьбе.
- Комментарии в коде — только когда действительно нужны.
- Владелец тестирует визуально на живом Hyprland; логи RUST_LOG=debug.

## Быстрые команды

```bash
make check   # fmt + clippy -D warnings
make build   # cargo build --release
make run     # запуск релизного бинарника
```

## Ключевые файлы

- docs/SPEC.md — что строим (не менять без владельца)
- docs/PLAN.md — как строим, чекбоксы прогресса (обновлять после каждой задачи)
