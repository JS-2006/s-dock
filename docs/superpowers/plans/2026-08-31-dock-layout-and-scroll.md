# s-dock: метрики дока, cap по ширине экрана, скролл иконок — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** починить стартовую отрисовку дока (лаунчер + пины + окна), гарантировать
запрос окон/пинов перед отрисовкой, ограничить ширину дока экраном и включить
горизонтальный скролл иконок (колесо + клавиатура) с всегда видимой кнопкой лаунчера.

**Architecture:** правки сосредоточены в `outputs.rs` (метрики/размеры поверхности)
и `app.rs` (layout с scrollable, cap по `screen_size`, wheel/keyboard-скролл через
widget-операции, gate «данные → отрисовка»). Сервис compositor получает
`fetch_snapshot()` для pull-запроса «перед отрисовкой»; пины уже грузятся из файла
синхронно до первого кадра и подхватываются watcher'ом.

**Tech Stack:** Rust 2024, iced_layershell fork v0.1.5 (iced 0.14 API), iced_runtime 0.14
(новая прямая зависимость для widget-операций scroll_by), Hyprland IPC (без изменений).

## Global Constraints

- Только Hyprland; стек и зависимости — как в `docs/PLAN.md` (Global Constraints).
- Тестов нет — качество: `make check` (cargo fmt --check + clippy -D warnings), ноль
  предупреждений, перед каждым коммитом.
- Коммиты: `fix(scope): …` / `feat(scope): …` / `docs: …`; ветка `main` (сейчас
  `feat/dock-ipc` — мерж по указанию владельца). Пушить только по просьбе.
- Язык ответов — русский; код — английский. Комментарии в коде — только где нужны.
- SPEC.md не менять без явной просьбы владельца (см. «Решения для подтверждения»).

## Диагноз (что найдено при изучении кода и логов)

1. **Лаунчер пропадает при >9 иконках** — `fit_content_size` (outputs.rs:411) считает
   слот `icon_size + 8` = 48px, а реальный footprint кнопки в `app_button` —
   `icon_size * hover_scale` = 60px (hover_scale=1.5 у владельца). Контент шире
   поверхности, ряд центрируется, края обрезаются — последним пропадает лаунчер
   (`launcher.position = "end"`), при большем числе иконок — крайние пины.
   Плюс лаунчер учтён дважды (в `sync_fit_size` уже добавлен к children, и ещё раз
   внутри `fit_content_size`).
2. **Точки/иконки обрезаются по вертикали** — `box_h = icon_size*hover_scale + dots`
   = 68px при `dock_size` = 50 и headroom 20: ряд центрируется в подложке 50px,
   нижние 9px (точки) уходят за нижний край поверхности.
3. **Стартовая пустота** — fallback-поверхность `SurfaceId::MAIN` (создаётся
   `.layer_shell()` в main.rs) не зарегистрирована: `Outputs::default()` даёт
   `fallback: None` → `view(MAIN)` возвращает пустой `row![]`, а `create()` её
   никогда не уничтожает (`replace_fallback = false`): невидимая поверхность
   200×70 с полным input region висит внизу по центру и перехватывает hover/клики.
   В ashell-ru (образец) fallback регистрируется как `SurfaceId::MAIN`.
4. **Нет «запроса перед отрисовкой»** — окна приходят только push-событиями;
   если стартовый fetch упал (сокет Hyprland не готов), retry нет, док рисуется из
   пустого состояния. Требование владельца: перед отрисовкой всегда запрос открытых
   окон и пинов.

## Решения для подтверждения владельцем (правки в план до старта)

- **A. Headroom заменяется на `dock_thickness = max(dock_size, slot_h)`.** Поверхность
  резервирует полный hover-размер кнопки (68px при конфиге владельца) вместо
  `dock_size + headroom` (70px) с центрированием. Эффект спеки #23 сохраняется:
  невидимая иконка 40px + точки сидят в подложке 50px, при hover иконка растёт вверх
  в прозрачную зону поверхности; соседи не сдвигаются (footprint фикс).
- **B. Скролл включается только когда контент реально шире экрана** (у владельца
  8 пинов ≈ 640px — скролла не будет).
- **C. Клавиатурный скролл работает, пока у поверхности дока фокус** — при переполнении
  dock-поверхности получают `KeyboardInteractivity::OnDemand` (фокус по клику на док),
  иначе `None`. Стрелки ←/→ листают по одной иконке.
- **D. «Запрос перед отрисовкой» трактуется как:** (1) первый fetch до первого
  контентного кадра (gate `windows_loaded`), (2) свежий fetch при показе дока
  (popup reveal), при добавлении выхода и смене конфига; пины — файл, читается
  синхронно в `App::new` и на live-правки watcher'ом.

---

### Task 1: Диагностика на живой машине (подтвердить диагноз)

**Files:** без правок кода.

**Interfaces:** подтверждённые гипотезы 1–4 из «Диагноза»; при расхождении — владелец
правит план до Task 2.

- [ ] **Step 1: Собрать и запустить с debug-логами**

```bash
cd ~/projects/s-dock && make build
RUST_LOG=debug ./target/release/s-dock 2>&1 | tee /tmp/s-dock-debug.log
```

- [ ] **Step 2: Проверить ключевые строки лога**

Ожидаемые строки и что они подтверждают:

- `Loaded 8 pins from ...` — пины прочитаны (сервис pins OK);
- `Fetched N windows` — стартовый запрос окон выполнен;
- `fit size -> (w, h)` — w должен быть ≈ `2*radius + icon_size + 8*slot(60) + gaps*8`;
  если w заметно меньше расчёта — подтверждён диагноз 1;
- `WARN s_dock::app: Pinned app not found: ...` — если есть, часть пинов не матчится;
- hover на полоску → dock появляется; если в центре низа экрана reveal не срабатывает,
  а по краям — да: подтверждён диагноз 3 (невидимая MAIN-поверхность перехватывает input).

- [ ] **Step 3: Зафиксировать выводы**

Записать фактические строки в этот план (ниже) и сверить с «Диагнозом».
При расхождениях — остановиться и согласовать правки плана с владельцем.

> **Результат (2026-08-31, по существующему логу `/tmp/s-dock/s-dock_rCURRENT.log`):**
> стартовый лог Info-уровня подтверждает стаб 200×50 (`size=(200,50)`) до fit-size,
> «Loaded 8 pins» есть; строк `fit size ->` и `Fetched N windows` в Info-логе нет
> (они debug) — запускающий проверяет их при `RUST_LOG=debug` в Task 6.
> Гипотезы 1–2 (метрики) подтверждены чтением кода; гипотеза 3 (MAIN-fallback)
> — чтением кода (`Outputs::default()` → `fallback: None`) и лога
> (MAIN не уничтожается — в логе нет его «Created»/destroy-следа).

---

### Task 2: Единые метрики кнопок и толщина дока (фикс пропажи лаунчера и обрезки)

**Files:**
- Modify: `src/outputs.rs` (`dock_layer_settings`, `fit_content_size`, новые `slot_metrics`, `dock_thickness`)
- Modify: `src/app.rs` (`sync_fit_size`, `view` — выравнивание ряда по краю экрана)

**Interfaces:**
- Produces: `outputs::slot_metrics(&AppearanceConfig) -> (f32 /*slot_w*/, f32 /*slot_h*/)`,
  `outputs::dock_thickness(&AppearanceConfig) -> u32`,
  `fit_content_size(&Config, &AppearanceConfig, groups: usize) -> (u32, u32)` —
  используется в Task 3 (cap) и Task 4 (шаг скролла).

- [ ] **Step 1: outputs.rs — добавить метрики и переписать расчёт fit-размера**

Заменить текущую `fit_content_size` целиком и добавить хелперы (рядом с ней):

```rust
/// Reserved (footprint) dimensions of one dock button. The same numbers are
/// used by `App::app_button`, so the surface size and the widget layout can
/// never diverge: slot_w = icon_size * max(hover_scale, 1), slot_h adds the
/// running-dots zone.
pub fn slot_metrics(appearance: &crate::config::AppearanceConfig) -> (f32, f32) {
    let icon = appearance.icon_size as f32;
    let scale = appearance.hover_scale.max(1.0);
    let dots = if appearance.show_running_indicator {
        appearance.running_indicator_size + 3.0
    } else {
        0.0
    };
    let slot_w = icon * scale;
    (slot_w, slot_w + dots)
}

/// Cross-axis size of the dock surface: at least the full button footprint,
/// so dots and the hover-scaled icon are never clipped by the surface bounds.
pub fn dock_thickness(appearance: &crate::config::AppearanceConfig) -> u32 {
    let (_, slot_h) = slot_metrics(appearance);
    (appearance.dock_size as f32).max(slot_h).round() as u32
}

/// Content-derived dock size for `Fit` mode. `groups` counts application
/// groups only; the launcher button (when enabled) is accounted here exactly once.
pub fn fit_content_size(
    config: &Config,
    appearance: &crate::config::AppearanceConfig,
    groups: usize,
) -> (u32, u32) {
    let horizontal = config.position.is_horizontal();
    let thickness = dock_thickness(appearance);
    let (slot_w, _) = slot_metrics(appearance);

    let launcher_w = if config.launcher.enabled {
        appearance.icon_size as f32
    } else {
        0.0
    };
    let items = groups as f32 + f32::from(config.launcher.enabled);
    let content = 2.0 * appearance.border_radius
        + launcher_w
        + groups as f32 * slot_w
        + (items.max(1.0) - 1.0) * appearance.spacing;
    let content = (content.round() as u32).max(thickness);

    if horizontal {
        (content, thickness)
    } else {
        (thickness, content)
    }
}
```

- [ ] **Step 2: outputs.rs — dock_layer_settings без headroom**

В `dock_layer_settings` заменить блок расчёта размеров (строки с `thickness`,
`headroom`, `(w, h)` и `size: Some((w, h + headroom))`):

```rust
let horizontal = config.position.is_horizontal();
let thickness = dock_thickness(appearance);
let stretch = appearance.width == WidthMode::Full;

let (w, h) = match (horizontal, stretch) {
    (true, true) => (0, thickness),
    (true, false) => (thickness * 4, thickness),
    (false, true) => (thickness, 0),
    (false, false) => (thickness, thickness * 4),
};
```

Удалить блок `let headroom = ...` и в `LayerShellSettings` вернуть
`size: Some((w, h))` (без `+ headroom`). Проверить, что `headroom` больше нигде
в файле не используется (`fit_content_size` уже переписан).

- [ ] **Step 3: outputs.rs — зарегистрировать MAIN-поверхность как fallback**

Сейчас `Outputs::default()` даёт `fallback: None`: поверхность `SurfaceId::MAIN`,
созданная `.layer_shell()` в main.rs, рендерит пустой `row![]`, никогда не
уничтожается и держит полный input region (перехватывает hover/клики у низа
экрана). Заменить `#[derive(Default)]` у `struct Outputs` на ручной impl:

```rust
#[derive(Debug)]
pub struct Outputs {
    surfaces: Vec<OutputDock>,
    /// `SurfaceId::MAIN` until real outputs replace it (mirrors ashell-ru).
    fallback: Option<SurfaceId>,
    /// At most one window-list popup at a time.
    window_popup: Option<(SurfaceId, Option<OutputId>)>,
    /// Last Fit-mode size; applied to any surface created after it was set.
    fit_size: Option<(u32, u32)>,
}

impl Default for Outputs {
    fn default() -> Self {
        Self {
            surfaces: Vec::new(),
            // Track the initial `.layer_shell()` surface as the fallback dock:
            // it must render content from the first frame and be replaced
            // (destroyed) when the first real output appears.
            fallback: Some(SurfaceId::MAIN),
            window_popup: None,
            fit_size: None,
        }
    }
}
```

(атрибуты над другими полями/структурой сохранить как есть; `#[derive(Default)]`
над `Outputs` убрать.) Проверить, что `Debug` нужен/выводим — если нет, оставить
`pub struct Outputs` без derive. После правки `view(MAIN)` рисует док сразу,
`create()` уничтожает fallback при первом выходе, `apply_input_regions`/
`apply_fit_size` включают MAIN в `dock_ids()`.

- [ ] **Step 4: app.rs — sync_fit_size передаёт только группы**

В `sync_fit_size` заменить строку с children (лаунчер больше не добавляется здесь —
он учтён внутри `fit_content_size`):

```rust
let groups = self.groups().len();
let size = use_theme(|a| crate::outputs::fit_content_size(&self.config, a, groups));
```

- [ ] **Step 5: app.rs — выровнять ряд по краю экрана (точки не обрезаются)**

В `view()` у контейнера-подложки дока (где сейчас `.align_x(Center)` /
`.align_y(Center)`) выровнять контент к экранному краю, чтобы переполнение
(увеличение иконки при hover) уходило ОТ экрана, а точки сидели на краю подложки:

```rust
let (align_x, align_y) = match self.config.position {
    config::Position::Top => {
        (iced::alignment::Horizontal::Center, iced::alignment::Vertical::Top)
    }
    config::Position::Bottom => {
        (iced::alignment::Horizontal::Center, iced::alignment::Vertical::Bottom)
    }
    config::Position::Left => {
        (iced::alignment::Horizontal::Start, iced::alignment::Vertical::Center)
    }
    config::Position::Right => {
        (iced::alignment::Horizontal::End, iced::alignment::Vertical::Center)
    }
};

let dock: Element<'_, Message> = container(content)
    .width(Length::Fill)
    .height(Length::Fixed(appearance.dock_size as f32))
    .padding([0.0, fit_padding])
    .align_x(align_x)
    .align_y(align_y)
    // ... style без изменений ...
```

- [ ] **Step 6: make check**

```bash
cd ~/projects/s-dock && make check
```
Expected: без ошибок, ноль предупреждений.

- [ ] **Step 7: Визуальная проверка владельцем**

Запустить (`RUST_LOG=debug`): док рисуется СРАЗУ при старте, без пустоты —
лаунчер (position=end), 8 пинов, окна; иконки и точки не обрезаны; при hover
иконка растёт вверх, соседи не двигаются; reveal по полоске работает по всему краю.

- [ ] **Step 8: Commit**

```bash
cd ~/projects/s-dock
git add src/outputs.rs src/app.rs
git commit -m "fix(dock): match surface size to real button footprint"
```

---

### Task 3: Cap ширины экраном + скролл иконок колесом, лаунчер всегда виден

**Files:**
- Modify: `src/app.rs` (`App` поля `icons_overflow`, message `IconScrollBy`, `view` layout, `sync_fit_size` + `cap_to_screen`, `OutputEvent::InfoChanged`)
- Modify: `Cargo.toml` (зависимость `iced_runtime = "0.14"`)

**Interfaces:**
- Consumes: `slot_metrics`/`dock_thickness`/`fit_content_size` (Task 2).
- Produces: `App::icons_overflow: bool` — признак переполнения, используется
  в Task 4 (клавиатура); `Message::IconScrollBy(f32)`.

- [ ] **Step 1: Cargo.toml — добавить iced_runtime**

В `[dependencies]` (рядом с `iced`):

```toml
iced_runtime = "0.14"
```

Разрешится в ту же версию 0.14.0, что уже в Cargo.lock у форка — типы совпадут.
Выполнить `cargo tree -i iced_runtime | head` и убедиться, что версия одна.

- [ ] **Step 2: app.rs — поля, сообщение и хелпер скролла**

В `struct App` добавить:

```rust
    /// Icon strip is wider than the screen: launcher is pinned, icons scroll.
    icons_overflow: bool,
```

(в `App::new` — `icons_overflow: false,`).

В `Message` добавить вариант и импорты:

```rust
    /// Wheel-driven horizontal scroll of the icon strip (px delta).
    IconScrollBy(f32),
```

Добавить константу и хелпер (уровень модуля app.rs):

```rust
const DOCK_SCROLL_ID: &str = "s-dock-icons";

fn scroll_icons(dx: f32) -> Task<Message> {
    let op = iced::core::widget::operation::scrollable::scroll_by(
        iced::widget::Id::new(DOCK_SCROLL_ID),
        iced::widget::scrollable::AbsoluteOffset { x: dx, y: 0.0 },
    );
    Task::from(iced_runtime::task::widget::<()>(op).map(|_| Message::None))
}
```

В `update()` добавить ветку (рядом с `Message::None`):

```rust
            Message::IconScrollBy(dx) => scroll_icons(dx),
```

- [ ] **Step 3: app.rs — cap fit-размера шириной экрана**

В `App` добавить метод и переписать `sync_fit_size`:

```rust
    /// Caps the computed size at the output's main-axis size (minus margins)
    /// and records whether the icon strip overflows the screen.
    fn cap_to_screen(&mut self, mut size: (u32, u32)) -> (u32, u32) {
        let Some((sw, sh)) = self.screen_size else {
            self.icons_overflow = false;
            return size;
        };
        let margin = self.config.appearance.margin.unsigned_abs() * 2;
        if self.config.position.is_horizontal() {
            let max_w = (sw as u32).saturating_sub(margin);
            self.icons_overflow = size.0 > max_w;
            size.0 = size.0.min(max_w);
        } else {
            let max_h = (sh as u32).saturating_sub(margin);
            self.icons_overflow = size.1 > max_h;
            size.1 = size.1.min(max_h);
        }
        size
    }

    fn sync_fit_size(&mut self) -> Task<Message> {
        let groups = self.groups().len();
        let size = use_theme(|a| crate::outputs::fit_content_size(&self.config, a, groups));
        let size = self.cap_to_screen(size);

        if self.config.appearance.width == config::WidthMode::Fit
            && Some(size) != self.applied_fit_size
        {
            debug!("fit size -> {size:?} overflow={}", self.icons_overflow);
            self.applied_fit_size = Some(size);
            self.recompute_dock_rect();
            return self.outputs.apply_fit_size(size).map(|_| Message::None);
        }
        Task::none()
    }
```

`screen_size` должен обновляться и при смене параметров выхода — в
`Message::OutputEvent` заменить ветку `InfoChanged`:

```rust
                OutputEvent::InfoChanged(info) => {
                    self.screen_size = info.logical_size;
                    self.refresh()
                }
```

- [ ] **Step 4: app.rs — layout: лаунчер вне скролла, иконки в scrollable**

В `view()` (ветка `SurfaceKind::Dock`) заменить сборку ряда (блок
`let (mut children, ...) ... let content = row(children)...`) на:

```rust
        let launcher_button = launcher.enabled.then(|| {
            let text_color = color_from_hex(launcher.text_color, launcher.opacity);
            button(
                text(launcher.label)
                    .size(appearance.icon_size as f32)
                    .width(Length::Fixed(appearance.icon_size as f32))
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .style(move |_, _status| iced::widget::button::Style {
                background: None,
                border: iced::Border::default().rounded(appearance.border_radius),
                text_color,
                ..Default::default()
            })
            .padding(0.0)
            .on_press(Message::LaunchLauncher)
            .into()
        });

        let icons_row: Element<'_, Message> = {
            let mut kids: Vec<Element<'_, Message>> = Vec::new();
            for group in self.groups() {
                kids.push(self.app_button(&group, id));
            }
            row(kids).spacing(appearance.spacing).into()
        };

        let icons: Element<'_, Message> = if self.icons_overflow {
            let scroll = scrollable(icons_row)
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::hidden(),
                ))
                .id(iced::widget::Id::new(DOCK_SCROLL_ID))
                .width(Length::Fill);
            container(mouse_area(scroll).on_scroll(|delta| match delta {
                iced::mouse::ScrollDelta::Lines { x: 0.0, y } => {
                    Message::IconScrollBy(-y * 60.0)
                }
                iced::mouse::ScrollDelta::Pixels { x: 0.0, y } => Message::IconScrollBy(-y),
                _ => Message::None,
            }))
            .width(Length::Fill)
            .into()
        } else {
            icons_row
        };

        let mut kids: Vec<Element<'_, Message>> = Vec::new();
        match launcher.position {
            config::LauncherPosition::Start => kids.extend(launcher_button),
            config::LauncherPosition::End => {}
        }
        kids.push(icons);
        match launcher.position {
            config::LauncherPosition::Start => {}
            config::LauncherPosition::End => kids.extend(launcher_button),
        }

        let content: Element<'_, Message> = row(kids).spacing(appearance.spacing).into();
```

Текущие переменные `children`/`trailing` и `match launcher.position` в начале
блока удалить. Фильтр `x: 0.0` оставляет вертикальное колесо под наш контроль
(горизонтальные дельты тачпада скроллит сам scrollable — двойного скролла нет).
Знак `-y * 60.0` — как в нативной обработке iced (`-movement * 60.0`); если на
живой машине колесо листает в обратную сторону — инвертировать знак в этом месте.
Импорты в `use iced::widget::{...}` дополнить: `scrollable`.

- [ ] **Step 5: make check**

```bash
cd ~/projects/s-dock && make check
```
Expected: ноль предупреждений.

- [ ] **Step 6: Визуальная проверка владельцем**

- 8 пинов: без скролла, вид как в Task 2.
- Добавить в `~/.config/s-dock/pins.toml` пины до ~30 иконок (или временно
  `icon_size = 64` в конфиге): ширина дока упирается в экран, лаунчер виден,
  колесо мыши над иконками листает ряд.

- [ ] **Step 7: Commit**

```bash
cd ~/projects/s-dock
git add Cargo.toml Cargo.lock src/app.rs
git commit -m "feat(dock): screen-width cap with wheel-scrollable icon strip"
```

---

### Task 4: Клавиатурный скролл (←/→) при переполнении

**Files:**
- Modify: `src/app.rs` (`Message::KeyPressed`, подписка `keyboard::listen`, ветка update)

**Interfaces:**
- Consumes: `App::icons_overflow` (Task 3), `scroll_icons(dx)` (Task 3).

- [ ] **Step 1: app.rs — сообщение и обработка клавиш**

В `Message` добавить:

```rust
    /// Raw key press; only Arrow keys are acted upon while overflowing.
    KeyPressed(iced::keyboard::Key),
```

В `update()`:

```rust
            Message::KeyPressed(key) => {
                if !self.icons_overflow {
                    return Task::none();
                }
                let horizontal = self.config.position.is_horizontal();
                let slot = use_theme(|a| a.icon_size as f32 * a.hover_scale.max(1.0));
                use iced::keyboard::key::Named;
                match (key, horizontal) {
                    (iced::keyboard::Key::Named(Named::ArrowRight), true)
                    | (iced::keyboard::Key::Named(Named::ArrowDown), false) => {
                        scroll_icons(slot)
                    }
                    (iced::keyboard::Key::Named(Named::ArrowLeft), true)
                    | (iced::keyboard::Key::Named(Named::ArrowUp), false) => {
                        scroll_icons(-slot)
                    }
                    _ => Task::none(),
                }
            }
```

- [ ] **Step 2: app.rs — подписка на клавиатуру**

В `App::subscription` в `Subscription::batch(vec![...])` добавить элемент:

```rust
            iced::keyboard::listen().map(|event| match event {
                iced::keyboard::Event::KeyPressed { key, .. } => Message::KeyPressed(key),
                _ => Message::None,
            }),
```

События приходят только когда какой-то слой имеет фокус — поэтому в Task 3
при переполнении dock-поверхностям выставляется `KeyboardInteractivity::OnDemand`
(фокус по клику на док).

- [ ] **Step 3: app.rs — OnDemand при переполнении**

В `struct App` добавить поле `keyboard_grabbed: bool` (в `App::new` — `false`).
Заменить `sync_fit_size` (версию из Task 3 Step 3) на итоговую форму — set_size
и переключение keyboard-interactivity собираются в один батч:

```rust
    fn sync_fit_size(&mut self) -> Task<Message> {
        let groups = self.groups().len();
        let size = use_theme(|a| crate::outputs::fit_content_size(&self.config, a, groups));
        let size = self.cap_to_screen(size);

        let mut tasks: Vec<Task<Message>> = Vec::new();

        if self.config.appearance.width == config::WidthMode::Fit
            && Some(size) != self.applied_fit_size
        {
            debug!("fit size -> {size:?} overflow={}", self.icons_overflow);
            self.applied_fit_size = Some(size);
            self.recompute_dock_rect();
            tasks.push(self.outputs.apply_fit_size(size).map(|_| Message::None));
        }

        if self.icons_overflow != self.keyboard_grabbed {
            self.keyboard_grabbed = self.icons_overflow;
            let ki = if self.icons_overflow {
                KeyboardInteractivity::OnDemand
            } else {
                KeyboardInteractivity::None
            };
            let ids: Vec<SurfaceId> = self.outputs.dock_ids().collect();
            tasks.extend(ids.into_iter().map(|id| {
                iced::set_keyboard_interactivity::<Message>(id, ki).map(|_| Message::None)
            }));
        }

        Task::batch(tasks)
    }
```

Импорты: `KeyboardInteractivity` в `use iced::{...}` (в app.rs его ещё нет),
`SurfaceId` уже импортирован.

- [ ] **Step 4: make check**

```bash
cd ~/projects/s-dock && make check
```
Expected: ноль предупреждений.

- [ ] **Step 5: Визуальная проверка владельцем**

При переполнении: клик по доку → стрелки ←/→ листают по иконке; после клика по
окну приложения фокус возвращается окну. Без переполнения клавиатура доку не
достаётся (поведение окон не меняется).

- [ ] **Step 6: Commit**

```bash
cd ~/projects/s-dock
git add src/app.rs
git commit -m "feat(dock): keyboard scrolling while the icon strip overflows"
```

---

### Task 5: Запрос окон/пинов перед отрисовкой

**Files:**
- Modify: `src/services/compositor.rs` (публичный `fetch_snapshot`)
- Modify: `src/app.rs` (`windows_loaded` gate, `fetch_task`, цепочки в
  `WindowsReady/WindowsUpdated`, `ConfigChanged`, `OutputEvent::Added`, `on_dock_entered`)
- Modify: `src/main.rs` (bootstrap fetch)

**Interfaces:**
- Consumes: `compositor::fetch_windows()` (уже есть, приватная).
- Produces: `compositor::fetch_snapshot() -> anyhow::Result<DockWindows>`;
  `App::windows_loaded: bool` — gate первого контентного кадра.

- [ ] **Step 1: compositor.rs — публичный one-shot запрос**

Рядом с `fetch_windows` добавить:

```rust
/// Fresh one-shot snapshot for the "query before render" flow: the dock must
/// never draw its icon row from data it has not explicitly requested.
pub async fn fetch_snapshot() -> anyhow::Result<DockWindows> {
    fetch_windows()
}
```

- [ ] **Step 2: app.rs — gate и fetch_task**

В `struct App` добавить поле `windows_loaded: bool` (в `App::new` — `false`).
В хендлерах `WindowsReady` и `WindowsUpdated` первой строкой:

```rust
                self.windows_loaded = true;
```

В `view()` (ветка `SurfaceKind::Dock`, сразу после проверки popup-скрытия):

```rust
        // Never draw icons from data that was not explicitly fetched.
        if !self.windows_loaded {
            return row![].into();
        }
```

Хелпер (в `impl App`):

```rust
    fn fetch_task(&self) -> Task<Message> {
        Task::perform(compositor::fetch_snapshot(), |result| match result {
            Ok(windows) => Message::WindowsUpdated(windows),
            Err(e) => {
                log::error!("Compositor fetch failed: {e}");
                Message::None
            }
        })
    }
```

- [ ] **Step 3: app.rs — свежий запрос перед показом/пересозданием поверхностей**

- `Message::ConfigChanged`: `recreate.map(|_| Message::None).chain(self.refresh())`
  дополнить `.chain(self.fetch_task())`;
- `Message::OutputEvent(added)`: `create.map(|_| Message::None).chain(self.refresh())`
  дополнить `.chain(self.fetch_task())`;
- `on_dock_entered`: ветку показа заменить на
  `self.apply_input_regions().chain(self.fetch_task())` (запрос перед каждым
  открытием дока; повторный вход на уже видимый док — без запроса).

- [ ] **Step 4: main.rs — bootstrap fetch**

В bootstrap-замыкании `iced::application(...)` заменить `(app, Task::none())`:

```rust
    iced::application(
        move || {
            let app = App::new((config.clone(), config_path.clone()), dock_rect.clone());
            let task = app.bootstrap();
            (app, task)
        },
        App::update,
        App::view,
    )
```

В `impl App`:

```rust
    /// First render must be preceded by an explicit windows query.
    pub fn bootstrap(&self) -> Task<Message> {
        self.fetch_task()
    }
```

- [ ] **Step 5: make check**

```bash
cd ~/projects/s-dock && make check
```
Expected: ноль предупреждений.

- [ ] **Step 6: Визуальная проверка владельцем**

Старт: `RUST_LOG=debug ./target/release/s-dock` — в логе `Fetched N windows`
и `fit size ->` появляются до первого кадра с иконками; док (hover) показывает
лаунчер+пины+окна сразу, без «пустого» промежутка. Выключить/включить Hyprland-события
(открыть/закрыть окна) — док обновляется как раньше.

- [ ] **Step 7: Commit**

```bash
cd ~/projects/s-dock
git add src/services/compositor.rs src/app.rs src/main.rs
git commit -m "feat(dock): fetch windows before every content render"
```

---

### Task 6: Финальная верификация и синхронизация docs/PLAN.md

**Files:**
- Modify: `docs/PLAN.md` (новый раздел этапа с чекбоксами, статус в шапке)

- [ ] **Step 1: полный чек-лист владельца**

```bash
cd ~/projects/s-dock && make check && make build
```

- запуск в `mode = "popup"`: hover полоски → док с лаунчером, 8 пинами и окнами;
- пины/окна не обрезаны, точки на месте при hover_scale = 1.5;
- ~30 иконок: ширина = экран минус margin, лаунчер виден, колесо листает;
- клик по доку при переполнении → ←/→ листают; после клика по окну — фокус у окна;
- resize/смена разрешения (InfoChanged) — cap пересчитывается;
- второй монитор: док на обоих выходах, поведение идентичное.

- [ ] **Step 2: внести этап в docs/PLAN.md**

Добавить в `docs/PLAN.md` раздел (чекбоксы проставить по факту):

```markdown
## Этап 9. Метрики дока, cap по экрану, скролл иконок

- [x] 9.1 Метрики кнопок = слот fit-расчёта (slot_w = icon_size*hover_scale),
      thickness = max(dock_size, slot_h), headroom убран, лаунчер считается один раз.
- [x] 9.2 Cap fit-ширины экраном (screen_size из OutputEvent, InfoChanged обновляет).
- [x] 9.3 Переполнение → scrollable иконок (колесо), лаунчер вне скролла.
- [x] 9.4 Клавиатурный скролл (←/→) при OnDemand-фокусе дока.
- [x] 9.5 Запрос окон перед отрисовкой (bootstrap fetch + gate + fetch на reveal).
```

- [ ] **Step 3: Commit**

```bash
cd ~/projects/s-dock
git add docs/PLAN.md
git commit -m "docs: stage 9 dock sizing and overflow scrolling"
```

## Вне скоупа этой итерации (известные ограничения)

- Вертикальные доки (`position = left/right`): view строит горизонтальный ряд,
  cap/скролл для них зеркалятся по формулам, но полный вертикальный layout —
  отдельная задача.
- Зеркалирование для `position = top` (точки над иконкой, hover вниз): сейчас
  точки всегда под иконкой — унаследованное поведение, отдельная полировка.
- Drag-and-drop внутри scrollable работает в видимой зоне; автоскролл при драге
  у края — не реализован (v2 при необходимости).
- Multi-output: `screen_size` берётся с последнего добавленного выхода; при
  сильно разных разрешениях cap будет по нему (уточнить при необходимости).
