use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::mpsc::channel;

use clap::{Parser, Subcommand};
use glob::glob;
use new_string_template::template::Template;
use regex::Regex;
use serde::{de::value::MapDeserializer, Deserialize};
use serde_derive::{Deserialize, Serialize};
use unity_rs::UnityError;

// common types

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
enum SupportedAssetType {
    #[serde(rename = "sprite")]
    Sprite,
    #[serde(rename = "texture2d")]
    Texture2D,
    #[serde(rename = "text")]
    TextAsset,
}

// types for argument parsing

#[derive(Debug, Parser)]
struct Args {
    #[clap(subcommand)]
    action: ArgsAction,
}

#[derive(Debug, Subcommand)]
enum ArgsAction {
    Extract {
        #[clap(short = 'd', long = "dry")]
        dry_run: bool,
        #[clap(short = 'r', long = "chdir")]
        chdir: bool,
        #[clap(short = 'i', long = "incremental")]
        incremental: bool,
        #[clap(short = 'c', long = "config")]
        config_file: String,
    },
    Inspect {
        #[clap(short = 's', long = "only-supported")]
        only_supported: bool,
        #[clap(required = true)]
        files: Vec<String>,
    },
}

// types for config file

#[derive(Debug, Deserialize)]
struct Config {
    /// The source file glob pattern
    src: String,
    /// The destination directory
    dest: String,
    /// The list of targets
    targets: Vec<ConfigTarget>,
}

#[derive(Debug, Deserialize)]
struct ConfigTarget {
    /// Type to extract
    r#type: SupportedAssetType,
    /// The template string to use as path pattern
    template: String,
    /// The regex to use to match the path pattern specified in `template`
    r#match: String,
    /// The regex replacement string to use to generate the destination path
    dest: String,
}

// types for asset bundle type tree

#[derive(Debug, Deserialize)]
struct PPtr {
    #[serde(rename = "m_PathID")]
    path_id: i64,
    #[serde(rename = "m_FileID")]
    file_id: i64,
}

#[derive(Debug, Deserialize)]
struct AssetBundle {
    #[serde(rename = "m_Name")]
    name: String,
    #[serde(rename = "m_AssetBundleName")]
    asset_bundle_name: String,
    #[serde(rename = "m_Container")]
    container: HashMap<String, AssetBundleContainer>,
    #[serde(rename = "m_PreloadTable")]
    preload_table: Vec<PPtr>,
}

#[derive(Debug, Deserialize)]
struct AssetBundleContainer {
    #[serde(rename = "asset")]
    asset: PPtr,
    #[serde(rename = "preloadIndex")]
    preload_index: u64,
    #[serde(rename = "preloadSize")]
    preload_size: u64,
}

// types for internal use

#[derive(Debug)]
struct AssetBundleInfo {
    container_name_map: HashMap<i64, String>,
}

#[derive(Debug)]
enum AssetMetadata {
    Supported(SupportedAssetType, String),
    Unsupported(String),
}

fn collect_asset_bundle_info(
    env: &unity_rs::Env,
) -> Result<AssetBundleInfo, Box<dyn std::error::Error>> {
    let mut container_name_map = HashMap::new();

    for obj in env.objects() {
        if obj.class() != unity_rs::ClassID::AssetBundle {
            continue;
        }

        let info = obj.info.read_type_tree()?;
        let asset_bundle = AssetBundle::deserialize(MapDeserializer::new(info.into_iter()))?;

        for (container_name, container) in asset_bundle.container {
            for i in container.preload_index..container.preload_index + container.preload_size {
                let path_id = match asset_bundle.preload_table.get(i as usize) {
                    Some(pptr) => pptr.path_id,
                    None => {
                        eprintln!("Failed to get preload_table[{}]", i);
                        continue;
                    }
                };
                container_name_map.insert(path_id, container_name.clone());
            }
        }
    }

    Ok(AssetBundleInfo { container_name_map })
}

fn get_asset_metadata(obj: &unity_rs::Object) -> Result<AssetMetadata, UnityError> {
    match obj.class() {
        unity_rs::ClassID::Sprite => {
            let sprite: unity_rs::classes::Sprite = obj.read()?;
            Ok(AssetMetadata::Supported(
                SupportedAssetType::Sprite,
                sprite.name,
            ))
        }
        unity_rs::ClassID::Texture2D => {
            let texture: unity_rs::classes::Texture2D = obj.read()?;
            Ok(AssetMetadata::Supported(
                SupportedAssetType::Texture2D,
                texture.name,
            ))
        }
        unity_rs::ClassID::TextAsset => {
            let text: unity_rs::classes::TextAsset = obj.read()?;
            Ok(AssetMetadata::Supported(
                SupportedAssetType::TextAsset,
                text.name,
            ))
        }
        _ => Ok(AssetMetadata::Unsupported(format!(
            "{:?} (unsupported)",
            obj.class()
        ))),
    }
}

fn dump_asset(path: &str, obj: &unity_rs::Object) -> Result<(), Box<dyn std::error::Error>> {
    match obj.class() {
        unity_rs::ClassID::Sprite => {
            let sprite: unity_rs::classes::Sprite = obj.read()?;
            sprite.decode_image()?.save(path)?;
        }
        unity_rs::ClassID::Texture2D => {
            let texture: unity_rs::classes::Texture2D = obj.read()?;
            texture.decode_image()?.save(path)?;
        }
        unity_rs::ClassID::TextAsset => {
            let text: unity_rs::classes::TextAsset = obj.read()?;
            std::fs::write(path, text.script)?;
        }
        _ => Err(UnityError::Unimplemented)?,
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Args::parse_from(wild::args()).action {
        ArgsAction::Extract {
            config_file,
            chdir,
            dry_run,
            incremental,
        } => {
            let config: Config =
                match toml::from_str(&match std::fs::read_to_string(&config_file) {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!("Failed to read config file: {}\n", e);
                        return Ok(());
                    }
                }) {
                    Ok(config) => config,
                    Err(e) => {
                        eprintln!("Failed to parse config file: {}\n", e);
                        return Ok(());
                    }
                };

            // config.toml -> config_progress.txt
            let incremental_progress_filename: Option<String> = if incremental {
                Some(format!(
                    "{}_progress.txt",
                    std::path::Path::new(&config_file)
                        .file_stem()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or_default()
                ))
            } else {
                None
            };

            let processed_files: HashSet<String> = match &incremental_progress_filename {
                Some(filename) => std::fs::read_to_string(filename)
                    .unwrap_or_default()
                    .lines()
                    .map(|line| line.to_string())
                    .collect(),
                None => HashSet::new(),
            };

            // Open progress file with append mode
            let mut incremental_progress_file = incremental_progress_filename.map(|filename| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(filename)
                    .unwrap()
            });

            if chdir {
                std::env::set_current_dir(
                    std::path::Path::new(&config_file)
                        .parent()
                        .expect("Failed to extract parent path from config filename"),
                )
                .expect("Failed to set current directory");
            }

            let targets_instantiated = config
                .targets
                .iter()
                .map(|target| {
                    let path_pattern = Template::new(&target.template);
                    let path_regex = Regex::new(&target.r#match)
                        .unwrap_or_else(|_| panic!("Failed to compile regex: {}", target.r#match));
                    let path_replacement = target.dest.clone();
                    (target.r#type, path_pattern, path_regex, path_replacement)
                })
                .collect::<Vec<_>>();

            let (tx, rx) = channel();
            ctrlc::set_handler(move || tx.send(()).expect("Could not send signal on channel."))
                .expect("Error setting Ctrl-C handler");

            for bundle_path in glob(&config.src)
                .unwrap_or_else(|_| panic!("Failed to glob: {}", &config.src))
                .flatten()
            {
                if rx.try_recv().is_ok() {
                    eprintln!("Interrupted");
                    break;
                }

                let str_bundle_path = bundle_path
                    .as_path()
                    .to_str()
                    .unwrap_or_default()
                    .replace('\\', "/");
                let should_skip = processed_files.contains(&str_bundle_path);
                println!(
                    "{}{}",
                    str_bundle_path,
                    if should_skip { " (skipped)" } else { "" }
                );

                if should_skip {
                    continue;
                }

                let mut env = unity_rs::Env::new();
                let data = match std::fs::read(&bundle_path) {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!("Failed to read file: {}\n", e);
                        continue;
                    }
                };
                if env.load_from_slice(&data).is_err() {
                    eprintln!("Failed to parse asset bundle\n");
                    continue;
                }
                let container_name_map = match collect_asset_bundle_info(&env) {
                    Ok(info) => info.container_name_map,
                    Err(e) => {
                        eprintln!("Failed to collect asset bundle info: {}\n", e);
                        continue;
                    }
                };
                for (index, obj) in env.objects().enumerate() {
                    let (r#type, name) = match get_asset_metadata(&obj) {
                        Ok(AssetMetadata::Supported(r#type, name)) => (r#type, name),
                        Ok(AssetMetadata::Unsupported(_)) => {
                            continue;
                        }
                        Err(e) => {
                            eprintln!("Failed to read object: {}\n", e);
                            continue;
                        }
                    };

                    let mut placeholder_map = HashMap::new();
                    placeholder_map.insert("name", name);
                    placeholder_map.insert(
                        "container",
                        container_name_map
                            .get(&obj.info.path_id)
                            .cloned()
                            .unwrap_or_default(),
                    );
                    placeholder_map.insert("index", index.to_string());
                    placeholder_map.insert("bundle_path", str_bundle_path.clone());

                    for (target_type, path_pattern, path_regex, path_replacement) in
                        &targets_instantiated
                    {
                        if *target_type != r#type {
                            continue;
                        }

                        let rendered = path_pattern.render_nofail(&placeholder_map);
                        if !path_regex.is_match(&rendered) {
                            continue;
                        }

                        let path = path_regex.replace(&rendered, path_replacement);
                        let path = std::path::Path::new(&config.dest)
                            .join(std::path::Path::new(path.as_ref()));
                        let str_path = path.to_str().unwrap_or_default();

                        println!("  {}", str_path.replace('\\', "/"));
                        if !dry_run {
                            match path.parent() {
                                Some(parent) => {
                                    if !parent.exists() {
                                        std::fs::create_dir_all(parent)?;
                                    }
                                }
                                None => {
                                    eprintln!("Failed to get parent path of {}", str_path);
                                }
                            }
                            if let Err(e) = dump_asset(str_path, &obj) {
                                eprintln!("Failed to dump asset to {}: {}\n", str_path, e);
                            }
                        }
                    }
                }

                if let Some(file) = &mut incremental_progress_file {
                    writeln!(file, "{}", str_bundle_path)?;
                    file.flush()?;
                }
            }

            println!("Done");
        }
        ArgsAction::Inspect {
            only_supported,
            files,
        } => {
            for file in files {
                println!("{}", file.replace('\\', "/"));

                let mut env = unity_rs::Env::new();
                let data = match std::fs::read(file) {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!("  Failed to read file: {}\n", e);
                        continue;
                    }
                };
                if env.load_from_slice(&data).is_err() {
                    eprintln!("  Failed to parse asset bundle\n");
                    continue;
                }
                let container_name_map = match collect_asset_bundle_info(&env) {
                    Ok(info) => info.container_name_map,
                    Err(e) => {
                        eprintln!("  Failed to collect asset bundle info: {}\n", e);
                        continue;
                    }
                };
                for (index, obj) in env.objects().enumerate() {
                    let (supported, str_type, name) = match get_asset_metadata(&obj) {
                        Ok(AssetMetadata::Supported(r#type, name)) => (
                            true,
                            serde_json::to_string(&r#type)
                                .unwrap_or_default()
                                .replace('"', ""),
                            name,
                        ),
                        Ok(AssetMetadata::Unsupported(str_type)) => {
                            (false, str_type, String::new())
                        }
                        Err(e) => {
                            eprintln!("Failed to read object: {}\n", e);
                            continue;
                        }
                    };

                    if only_supported && !supported {
                        continue;
                    }

                    println!("  #{}: {}", index, str_type);
                    println!("    name: {}", name);
                    println!(
                        "    container: {}",
                        container_name_map
                            .get(&obj.info.path_id)
                            .unwrap_or(&String::new())
                    );
                }
            }

            println!("Done");
        }
    }

    Ok(())
}
