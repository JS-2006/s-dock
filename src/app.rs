use log::info;
use std::path::PathBuf;

use iced::{
    Element, Length, OutputEvent, Subscription, SurfaceId, Task, output_events,
    widget::{button, row, text},
};

use crate::{config, outputs::Outputs, theme::use_theme};

pub struct App {
    config: config::Config,
    config_path: PathBuf,
    outputs: Outputs,
}

#[derive(Debug, Clone)]
pub enum Message {
    ConfigChanged(Box<config::Config>),
    OutputEvent(OutputEvent),
    LaunchLauncher,
    None,
}

impl App {
    pub fn new((config, config_path): (config::Config, PathBuf)) -> Self {
        Self {
            config,
            config_path,
            outputs: Outputs::default(),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ConfigChanged(config) => {
                info!("Config reloaded");
                self.config = *config;
                crate::theme::update(&self.config.appearance);

                // Geometry-affecting options require fresh surfaces.
                self.outputs
                    .recreate_all(&self.config)
                    .map(|_| Message::None)
            }
            Message::OutputEvent(event) => match event {
                OutputEvent::Added(info) => self
                    .outputs
                    .create(&self.config, info.name.clone(), info.id)
                    .map(|_| Message::None),
                OutputEvent::Removed(output_id) => self
                    .outputs
                    .remove(&self.config, output_id)
                    .map(|_| Message::None),
                OutputEvent::InfoChanged(_) => Task::none(),
            },
            Message::None => Task::none(),
            Message::LaunchLauncher => {
                let cmd = self.config.launcher.cmd.clone();
                if !cmd.is_empty() {
                    tokio::spawn(async move { crate::utils::launcher::execute(cmd).await });
                }
                Task::none()
            }
        }
    }

    pub fn view(&self, id: SurfaceId) -> Element<'_, Message> {
        if !self.outputs.is_dock(id) {
            return row![].into();
        }

        let appearance = use_theme(|a| a.clone());

        let launcher = self.config.launcher.clone();

        let launcher_button = launcher.enabled.then(|| {
            let text_color = color_from_hex(launcher.text_color);
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

        let (mut children, mut trailing): (Vec<Element<'_, Message>>, Vec<Element<'_, Message>>) =
            (Vec::new(), Vec::new());
        match launcher.position {
            config::LauncherPosition::Start => {
                children.extend(launcher_button);
            }
            config::LauncherPosition::End => {
                trailing.extend(launcher_button);
            }
        }

        children.extend(trailing);

        let content: Element<'_, Message> = row(children).spacing(appearance.spacing).into();

        iced::widget::container(content)
            .width(match appearance.width {
                config::WidthMode::Full => Length::Fill,
                config::WidthMode::Fit => Length::Fill,
            })
            .height(Length::Fixed(appearance.dock_size as f32))
            .style(move |theme: &iced::Theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(color_from_hex(
                    appearance.background_color,
                ))),
                border: iced::Border {
                    color: color_from_hex(appearance.border_color),
                    width: 1.0,
                    radius: (appearance.border_radius).into(),
                },
                text_color: Some(theme.palette().text),
                ..Default::default()
            })
            .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            config::subscription(&self.config_path).map(|m| match m {
                config::Message::ConfigChanged(c) => Message::ConfigChanged(c),
            }),
            output_events().map(Message::OutputEvent),
        ])
    }
}

fn color_from_hex(hex: hex_color::HexColor) -> iced::Color {
    iced::Color::from_rgba8(hex.r, hex.g, hex.b, hex.a as f32 / 255.0)
}
