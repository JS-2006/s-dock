# s-dock — Implementation Plan

> ПРОТОКОЛ ВОССТАНОВЛЕНИЯ СЕССИИ: (1) прочитай docs/SPEC.md и этот файл целиком;
> (2) найди первый непоставленный чекбокс `- [ ]` — это текущая задача; (3) работай
> строго по одной задаче, после завершения поставь `x`, обнови «Статус» в шапке
> и сделай коммит (коммиты на этом проекте согласованы с владельцем).
> Не пересматривай утверждённые решения из SPEC.md.

**Статус:** этапы 1–3 код готов + досрочно: список окон (часть 6), мульти-точки.
НЕ сделано: 4 (drag-and-drop) — СЛЕДУЮЩАЯ ЗАДАЧА; 5 (hover ×1.3) — сразу после 4;
пиксельные миниатюры окон (нужен ext-image-copy-capture, согласовать отдельно).
В этапе 7 останется анимация выезда и пер-мониторная видимость. (визуальные проверки владельцем по ходу).
Досрочно сделано: 3.3 (лаунчер), большая часть 7 (хотспот+автоскрытие).
Следующая задача: 4.1 (drag-and-drop). В этапе 7 останется анимация выезда и
пер-мониторная синхронизация видимости.

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

## Этап 1. Каркас + конфиг + окно дока [ГОТОВ]

- [x] 1.1 Скелет репозитория: Cargo.toml (зависимости из Global Constraints;
      profile.release lto thin/strip/opt3/panic abort как в ashell-ru),
      Makefile (`check`=fmt --check +
      clippy -D warnings, `build`, `start`), .gitignore (/target).
- [x] 1.2 `src/config.rs`: все типы из эскиза конфига SPEC.md
      (`Position`, `Alignment`, `Mode`, `WidthMode`, `HotspotWidth{Dock,Pixels}`,
      AppearanceConfig, PopupConfig, LauncherConfig, Config с `#[serde(default)]`),
      загрузка `~/.config/s-dock/config.toml` (+ `--config-path` через clap),
      дефолты ровно как в эскизе, hot-reload watcher (inotify → Message::ConfigChanged).
- [x] 1.3 `src/theme.rs`: глобальный appearance из конфига (`init`/`update`/
      `use_theme`), контейнер дока: background_color, border_radius, border_color.
- [x] 1.4 `src/main.rs` + `src/app.rs` + `src/outputs.rs`: bootstrap по паттерну
      ashell; fallback-поверхность MAIN заменяется пер-выходными при
      OutputEvent::Added и восстанавливается при исчезновении всех выходов;
      anchor по position|alignment (center без stretch = без якоря на оси),
      exclusive_zone = dock_size при Fixed / 0 при Popup; width Full = растяжение,
      Fit = заглушка thickness*4; hot-reload пересоздаёт поверхности.
- [x] 1.5 Проверка: `make check` зелёный; владелец подтвердил работу подложки.
      Коммит `feat: project scaffold with config and dock window`.
- [x] 1.7 Правки владельца: автоскрытие popup-режима (`popup.autohide_timeout`,
      сек, дефолт 3, 0=off) + хотспот-полоска как отдельная поверхность
      (outputs.rs: strip_layer_settings, SurfaceKind::Strip); состояния
      App.popup_visible/hide_generation, MouseArea enter/exit,
      apply_input_regions() (скрытый док = пустой input region, полоска =
      активна; наоборот при показе). Коммит
      `feat(dock): popup auto-hide with configurable timeout and hotspot strip`.
      См. также правки прозрачности (appearance.opacity, launcher.opacity)
      и фикс центрирования кластера — коммит
      `feat(dock): configurable element opacity, center dock cluster`.
- [x] 1.6 Правки владельца после проверки: popup = Overlay + exclusive −1
      (поверх окон, без сдвига), fixed = Top + exclusive dock_size;
      кнопка лаунчера отрисована раньше срока (этап 3.3): позиция
      `launcher.position = start|end`, цвет текста `launcher.text_color`,
      клик запускает `launcher.cmd` через sh -c (utils/launcher.rs).
      Коммит `feat(dock): launcher button and layer behavior per mode`.

## Этап 2. Приложения и окна Hyprland

- [x] 2.1 hyprland_ipc.rs перенесён из ashell-ru без изменений;
      services/compositor.rs: broadcaster + AsyncEventListener
      (windowopened/closed/moved, activewindow, activemonitor, workspacechanged)
      → полный refetch через crate (Clients::get, сортировка по focus_history_id);
      DockWindows { windows, active }; Message::{Ready, Updated}.
- [x] 2.2 `src/services/apps.rs`: freedesktop-desktop-entry 0.8 (Iter.entries),
      AppRegistry с индексом match_keys (appid-stem, icon-name, name, lowercase);
      иконки freedesktop_icons::lookup + тема linicon_theme::get_icon_theme,
      кэш путей в App (RefCell).
- [x] 2.3 Группировка по классу (AppGroup); ЛКМ: нет окон → запуск очищенного
      Exec (sh -c), есть → pick_focus_target (активное окно группы иначе последнее
      по focus history) → focus_window() через hyprland_ipc (Lua/Legacy).
      Точка running indicator под иконкой (appearance.show_running_indicator,
      цвет/размер из конфига). Пины добавятся в этапе 3.
- [x] 2.5 (досрочно, правки владельца): иконки — фолбэк absolute-path →
      лестница размеров → unthemed → pixmaps; промахи не кэшируются;
      точки по количеству окон (до 10); ЛКМ при >1 окнах — попап списка окон
      (SurfaceKind::WindowList, outputs.window_list_settings, WindowPopup),
      клик по строке = фокус, закрытие: повторный клик/уход курсора/все окна
      закрыты. Коммит `feat(dock): window list popup, per-window dots, robust icons`.
- [ ] 2.4 Визуальная проверка владельцем: окна появляются/исчезают, клик =
      фокус/запуск, точки видны, иконки резолвятся (иначе буква-заглушка).
      Коммит `feat(dock): app entries, window tracking and launching` выполнен.

## Этап 3. Пины и лаунчер

- [x] 3.1 state.rs: pins.toml (Vec<desktop-id> в порядке отображения),
      атомарная запись (tmp+rename), загрузка при старте; live-редактирование
      файла через новый utils/watch.rs (обобщённый watcher, config переведён на него).
- [x] 3.2 Группировка перестроена: пины всегда в доке по порядку файла,
      окна «прилипают» к пину через match_keys; затем незакреплённые запущенные.
      UI открепления придёт с меню (этап 6) — Message::TogglePin добавлен тогда же.
      Пока пользователь может править pins.toml вручную (подхватывается live).
- [ ] 3.3 Кнопка лаунчера слева: enabled/label/cmd из конфига, клик → tokio::process cmd.
      Коммит `feat(dock): pinned apps state and launcher button`.

## Этап 4. Drag-and-drop [ГОТОВ]

- [x] 4.1 Перестановка пинов: MouseArea-press/move/release, автопрокрутка не нужна;
      сохранение нового порядка в pins.toml.
- [x] 4.2 Вытягивание за пределы дока при активном драге → немедленный запуск
      (DockLeftOrDragOut); ghost-иконку под курсором отложили (v2).
      Клик отделён от драга через press/release: release на той же иконке =
      клик (фокус/запуск/попап). Коммит
      `feat(dock): stages 4+5 drag-and-drop and hover scale`.
- [ ] Проверка владельцем: reorder переживает перезапуск; вытянул → новое окно.

## Этап 5. Фокус-эффект [ГОТОВ]

- [x] 5.1 Hover: масштаб иконки до appearance.hover_scale; фиксированный
      footprint кнопки (box_w/box_h по максимуму) — соседи не сдвигаются;
      headroom в поверхности горизонтальных доков ((scale−1)·icon_size),
      визуал прижат к краю экрана (align_y Bottom/Top). Известное
      ограничение: headroom ловит клики (уточнить input region позже).

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

## Этап 7. Popup-режим [ГОТОВ]

- [x] 7.3 Анимация выезда: slide к краю экрана (~150 мс, ANIM_STEP), тики
      только пока есть анимации (time::every 16ms условно в subscription);
      завершение скрытия снимает видимость и пересинхронизирует input region.
- [x] 7.2 Пер-мониторная видимость: visible_docks/hide_generations keyed by
      SurfaceId; strip будит только свой dock (dock_for_strip);
      apply_input_regions по паре (dock,strip). (hotspot_width/hotspot_height),
      anchor как у дока; MouseArea enter → показать док (анимация slide:
      изменение margin/size по кадрам через time::every или анимация iced),
      leave дока → скрыть, вернуть полоску.
- [ ] Визуальная проверка владельцем (анимация, независимость мониторов).

## Этап 8. Финализация [ГОТОВ]

- [x] README.md, docs/config-example.toml дополнен полями popup-меню,
      docs/pins-example.toml, CHANGELOG.md.

## Известные риски

- Drag-out требует выхода указателя за пределы поверхности → решается расширением
  поверхности/input-region на время драга (этап 4.2); fallback: зона запуска =
  область над доком.
- Анимации в iced_layershell: если плавность мешает — сначала дискретные шаги.
- j/clients JSON-схема может отличаться в новых версиях — проверять на живой машине
  владельца (RUST_LOG=debug).
