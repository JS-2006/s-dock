# Настраеваемый док-бар для Hyprland, на Rust.
## Фичи
- изменение положения (верх, низ, справа, слева)
- включить/отключить кнопку лаунчера
- автоскрытие
-управление окнами
## Скрииншоты
<img width="1252" height="130" alt="image" src="https://github.com/user-attachments/assets/6ca81b8d-31df-4e20-be08-d388793b95c0" />
## Сборка
```
git clone https://github.com/JS-2006/s-dock.git
cd s-dock
cargo build --release
```
## Пример конфигурации
~/.config/s-dock/config.toml
```
# Пример конфигурации s-dock
# Положить в ~/.config/s-dock/config.toml
# Все поля опциональны — отсутствующие берут значения по умолчанию.

log_level = "info"          # warn|info|debug|trace

position = "bottom"         # bottom | top | left | right
alignment = "center"        # center | start | end
mode = "popup"              # fixed (резервирует место) | popup (по хотспоту)

[appearance]
icon_size = 40
dock_size = 50            # толщина дока, px
width = "fit"               # fit (по контенту) | full (весь экран)
margin = 6
spacing = 8
opacity = 0.7          # прозрачность подложки дока, 0..1
background_color = "#BBC8F0"
border_radius = 25
border_color = "#89b4fa00"
hover_scale = 1.5           # увеличение иконки под курсором
show_running_indicator = true
running_indicator_color = "#FADADD"
running_indicator_size = 5

[popup]
hotspot_width = "dock"      # "dock" — во всю ширину края, или число px
hotspot_height = 2
autohide_timeout = 1        # сек без курсора на доке до скрытия; 0 = не скрывать

[launcher]
enabled = false
label = "󱗼"                # эмодзи или текст
cmd = "s-launcher toggle"                    # напр. "fuzzel"
position = "end"          # start | end — где кнопка в ряду дока (относительно приложений)
text_color = "#2E3440"
opacity =0.7               # прозрачность надписи кнопки, 0..1
```
