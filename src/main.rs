use clap::Parser;
use flexi_logger::{
    Age, Cleanup, Criterion, FileSpec, LogSpecBuilder, LogSpecification, Logger, Naming,
};
use iced::Task;
use log::{error, warn};
use std::{backtrace::Backtrace, panic, path::PathBuf};

mod app;
mod config;
mod outputs;
mod services;
mod theme;
mod utils;

const TMP_FILE_SIZE: u64 = 10 * 1024 * 1024;

#[derive(Parser, Debug)]
#[command(version, about = env!("CARGO_PKG_DESCRIPTION"))]
struct Args {
    #[arg(short, long, value_parser = clap::value_parser!(PathBuf))]
    config_path: Option<PathBuf>,
}

fn get_log_spec(log_level: &str) -> LogSpecification {
    let new_spec = LogSpecification::env_or_parse(log_level);

    match new_spec {
        Ok(spec) => spec,
        Err(err) => {
            warn!("Failed to parse log level: {err}, use the default");

            LogSpecification::default()
        }
    }
}

fn main() -> iced::Result {
    let args = Args::parse();

    let logger = Logger::with(
        LogSpecBuilder::new()
            .default(log::LevelFilter::Info)
            .build(),
    )
    .log_to_file(FileSpec::default().directory("/tmp/s-dock"))
    .duplicate_to_stdout(flexi_logger::Duplicate::All)
    .rotate(
        Criterion::AgeOrSize(Age::Day, TMP_FILE_SIZE),
        Naming::Timestamps,
        Cleanup::KeepLogFiles(7),
    )
    .start()
    .unwrap_or_else(|e| {
        eprintln!("Failed to initialize file logger: {e}, falling back to stderr-only");
        Logger::with(
            LogSpecBuilder::new()
                .default(log::LevelFilter::Info)
                .build(),
        )
        .start()
        .expect("critical: cannot initialize any logger")
    });

    panic::set_hook(Box::new(|info| {
        let b = Backtrace::capture();
        error!("Panic: {info} \n {b}");
    }));

    let (config, config_path) = config::get_config(args.config_path).unwrap_or_else(|err| {
        error!("Failed to read config: {err}");

        std::process::exit(1);
    });

    logger.set_new_spec(get_log_spec(&config.log_level));

    theme::init(&config.appearance);

    let initial_layer_settings = outputs::dock_layer_settings(&config, None);

    iced::application(
        move || {
            let app = App::new((config.clone(), config_path.clone()));
            (app, Task::none())
        },
        App::update,
        App::view,
    )
    .layer_shell(initial_layer_settings)
    .subscription(App::subscription)
    .theme(|_| iced::Theme::Dark)
    .run()
}

use app::App;
