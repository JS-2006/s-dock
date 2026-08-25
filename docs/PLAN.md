# s-dock — Implementation Plan

> ПРОТОКОЛ ВОССТАНОВЛЕНИЯ СЕССИИ: (1) прочитай docs/SPEC.md и этот файл целиком;
> (2) найди первый непоставленный чекбокс `- [ ]` — это текущая задача; (3) работай
> строго по одной задаче, после завершения поставь `x`, обнови «Статус» в шапке
> и сделай коммит (коммиты на этом проекте согласованы с владельцем).
> Не пересматривай утверждённые решения из SPEC.md.

**Статус:** этап 1 в работе. Готово: 0/8 этапов.

## Global Constraints

- Только Hyprland; стек = как у ashell-ru (`~/projects/ashell-ru`): Rust 2024,
  edition, MSRV 1.89, iced fork `iced_layershell` git tag v0.1.5 (те же features:
  tokio, advanced, wgpu, tiny-skia, image, svg, canvas), tokio, serde+toml,
  anyhow, log/flexi_logger, clap, inotify, freedesktop-icons, linicon-theme,
  hex_color, libc.
- Тестов нет — качество: `cargo fmt --check`, `cargo clippy -- -D warnings`
  (ноль предупреждений), сборка. Makefile-аналог `make check`.
- Коммиты: `feat(scope): …` / `fix(scope): …` / `chore: …`. Ветвь `main`.
- Язык ответов владельцу — русский; код/идентификаторы — английский.
- Комментарии в коде не писать без необходимости (правило ashell-ru).

## Технические паттерны из ashell-ru (копировать дословно)

### Bootstrap layer-shell приложения (src/main.rs ashell-ru)
```rust
iced::application(App::new(...), App::update, App::view)
    .layer_shell(LayerShellSettings {
        anchor: Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
        layer: Layer::Top,
        exclusive_zone: height as i32,
        size: Some((0, height as u32)),
        keyboard_interactivity: KeyboardInteractivity::None,
        namespace: "s-dock".into(),
        ..Default::default()
    })
    .subscription(App::subscription)
    .theme(App::theme)
    .scale_factor(App::scale_factor)
    .run()
```

### Дополнительные поверхности (multi-output, popups)
```rust
use iced::{new_layer_surface, destroy_layer_surface, set_anchor, set_size,
           set_exclusive_zone, set_input_region, set_keyboard_interactivity,
           LayerShellSettings, OutputId, SurfaceId};
let (id, task) = new_layer_surface(LayerShellSettings {
    namespace: "...".into(),
    size: Some((w, h)),
    anchor: ...,
    exclusive_zone: -1,      // поверх всего, не резервирует место
    output: Some(output_id), // привязка к выходу
    ..Default::default()
});
```
Мониторинг выходов: подписка `iced::OutputEvent` (`OutputEvent::Added(info)` /
`Removed(output_id)`) — см. src/app.rs:503-530 ashell-ru. SurfaceId::MAIN —
fallback-поверхность до детекта выходов.

### Hyprland IPC (перенести файл целиком)
Источник: `ashell-ru/src/services/compositor/hyprland_ipc.rs` — уже умеет
Lua/Legacy probe + `send_and_check` / `eval_dispatch` / `legacy_dispatch`.

---

## Этап 1. Каркас + конфиг + окно дока [В РАБОТЕ]

- [ ] 1.1 Скелет репозитория: Cargo.toml (зависимости из Global Constraints;
      profile.release lto thin/strip/opt3/panic abort как в ashell-ru),
      rustfmt.toml если есть в ashell-ru, Makefile (`check`=fmt --check +
      clippy -D warnings, `build`, `run`), .gitignore (/target), README-заглушка.
- [ ] 1.2 `src/config.rs`: все типы из эскиза конфига SPEC.md
      (`Position{Bottom,Top,Left,Right}`, `Alignment{Center,Start,End}`,
      `Mode{Fixed,Popup}`, `WidthMode{Fit,Full}`, `HotspotWidth{Dock,Pixels(u32)}`,
      Appearance, PopupConfig, LauncherConfig, Config top-level с `#[serde(default)]`),
      загрузка `~/.config/s-dock/config.toml` (+ `--config-path` через clap),
      дефолты ровно как в эскизе, hot-reload watcher (inotify → Task<Message>,
      паттерн config.rs ashell-ru).
- [ ] 1.3 `src/theme.rs`: минимальная тема — struct Space/Sizes/Colors из конфига
      appearance, функция `use_theme`-подобный доступ; контейнер дока:
      background_color, border_radius, border_color.
- [ ] 1.4 `src/main.rs` + `src/app.rs` + `src/outputs.rs`: bootstrap по паттерну
      выше; на каждый OutputEvent::Added — поверхность дока
      (anchor по position|alignment, exclusive_zone = dock_size при Fixed,
      0/-1 при Popup, size по width_mode: full=(0, dock_size) c anchor L|R,
      fit=(вычислить, dock_size)); рендер пустого ряда-заглушки.
      Alignment→anchor маппинг: center=LEFT|RIGHT, start=только ближний край,
      end=только дальний.
- [ ] 1.5 Проверка: `make check` зелёный; запуск под Hyprland — пустая подложка
      дока снизу по центру на каждом мониторе; fixed резервирует место
      (окна не залезают), popup — нет. Коммит `feat: project scaffold with config and dock window`.

## Этап 2. Приложения и окна Hyprland

- [ ] 2.1 Перенести hyprland_ipc.rs; сервис compositor: подписка socket2
      (openwindow/closewindow/activewindow/focusedmon) + периодический полный
      refetch `j/clients`; тип WindowInfo {address, class, title, focused}.
- [ ] 2.2 `src/services/apps.rs`: парсинг .desktop (`freedesktop-desktop-entry`),
      сопоставление class↔entry (StartupWMClass/name, lowercase match),
      иконки через freedesktop-icons (theme из конфига позже, GTK theme сейчас).
- [ ] 2.3 Модель дока: список групп AppEntry {icon, class, windows[], pinned?};
      ЛКМ: нет окон → launch (exec из entry, через tokio::process);
      есть → фокус `hl.dsp.focus({window="addr"})` последнего активного.
      Точка running indicator под иконкой (конфиг). Порядок: пины затем запущенные.
- [ ] 2.4 Проверка: реальные окна появляются/исчезают, клики работают, точки видны.
      Коммит `feat(dock): app entries, window tracking and launching`.

## Этап 3. Пины и лаунчер

- [ ] 3.1 state.rs: pins.toml (список desktop-file-id/class в порядке отображения),
      атомарная запись, загрузка при старте.
- [ ] 3.2 Закрепление/открепление (пока программно + через меню-заглушку),
      пины всегда в доке; порядок: пины по pins.toml, потом незакреплённые запущенные.
- [ ] 3.3 Кнопка лаунчера слева: enabled/label/cmd из конфига, клик → tokio::process cmd.
      Коммит `feat(dock): pinned apps state and launcher button`.

## Этап 4. Drag-and-drop

- [ ] 4.1 Перестановка пинов: MouseArea-press/move/release, автопрокрутка не нужна;
      сохранение нового порядка в pins.toml.
- [ ] 4.2 Вытягивание за пределы дока → запуск экземпляра (при press запоминаем
      иконку, рисуем её под курсором, release вне подложки → launch).
      Поверхность дока заранее больше ряда на hover_scale-вынос (см. этап 5),
      при активном драге расширять input region/поверхность (set_size/set_input_region).
- [ ] Проверка: reorder переживает перезапуск; вытянул → открылось новое окно.
      Коммит `feat(dock): drag and drop reordering and drag-out launching`.

## Этап 5. Фокус-эффект

- [ ] 5.1 hover: анимация scale до appearance.hover_scale (1.3) с выходом за край
      подложки; поверхность больше видимого ряда на величину выноса
      (margin_top = icon_size*(hover_scale-1)), клип-регион чтобы прозрачная зона
      не ловила клики мимо иконок (set_input_region по списку rect'ов иконок).
- [ ] Проверка визуально у владельца. Коммит `feat(dock): hover scale effect`.

## Этап 6. Контекстное меню

- [ ] 6.1 Отдельная layer-surface у иконки (паттерн menu.rs ashell-ru:
      new_layer_surface + ButtonUIRef/позиция от кнопки).
- [ ] 6.2 Пункты: закрепить/открепить (toggle по состоянию), открыть, закрыть
      (close всех окон группы: `hl.dsp.window.close("addr")` на каждое),
      разделитель, список окон этого приложения (title усечённый) → клик = фокус.
- [ ] Проверка: закрыть/переключить окно из меню. Коммит `feat(dock): context menu`.

## Этап 7. Popup-режим

- [ ] 7.1 Полоска-хотспот отдельной поверхностью (hotspot_width/hotspot_height),
      anchor как у дока; MouseArea enter → показать док (анимация slide:
      изменение margin/size по кадрам через time::every или анимация iced),
      leave дока → скрыть, вернуть полоску.
- [ ] 7.2 Координация per-output в outputs.rs (dock surface + hotspot surface).
- [ ] Проверка у владельца. Коммит `feat(dock): popup mode with hotspot strip`.

## Этап 8. Финализация

- [ ] README.md (установка, конфиг), пример конфига в docs/, CHANGELOG.
      Коммит `docs: readme and example config`.

## Известные риски

- Drag-out требует выхода указателя за пределы поверхности → решается расширением
  поверхности/input-region на время драга (этап 4.2); fallback: зона запуска =
  область над доком.
- Анимации в iced_layershell: если плавность мешает — сначала дискретные шаги.
- j/clients JSON-схема может отличаться в новых версиях — проверять на живой машине
  владельца (RUST_LOG=debug).
