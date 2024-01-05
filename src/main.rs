use std::collections::HashMap;

use clap::{Parser, Subcommand};
use glob::glob;
use new_string_template::template::Template;
use regex::Regex;
use serde::{de::value::MapDeserializer, Deserialize};
use serde_derive::Deserialize;
use unity_rs::UnityError;

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
    r#type: ConfigTargetType,
    /// The template string to use as path pattern
    template: String,
    /// The regex to use to match the path pattern specified in `template`
    r#match: String,
    /// The regex replacement string to use to generate the destination path
    dest: String,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
enum ConfigTargetType {
    #[serde(rename = "sprite")]
    Sprite,
    #[serde(rename = "texture2d")]
    Texture2D,
    #[serde(rename = "text")]
    TextAsset,
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

#[derive(Debug, Deserialize)]
struct PPtr {
    #[serde(rename = "m_PathID")]
    path_id: i64,
    #[serde(rename = "m_FileID")]
    file_id: i64,
}

#[derive(Debug)]
struct AssetBundleInfo {
    container_name_map: HashMap<i64, String>,
}

#[derive(Debug)]
enum AssetMetadata {
    Supported(ConfigTargetType, String),
    Unsupported(String),
}

fn has_supported_asset_bundle_signature(data: &[u8]) -> bool {
    data.len() > 8 && &data[0..8] == b"UnityFS\0"
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
                ConfigTargetType::Sprite,
                sprite.name,
            ))
        }
        unity_rs::ClassID::Texture2D => {
            let texture: unity_rs::classes::Texture2D = obj.read()?;
            Ok(AssetMetadata::Supported(
                ConfigTargetType::Texture2D,
                texture.name,
            ))
        }
        unity_rs::ClassID::TextAsset => {
            let text: unity_rs::classes::TextAsset = obj.read()?;
            Ok(AssetMetadata::Supported(
                ConfigTargetType::TextAsset,
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
                        .expect(format!("Failed to compile regex: {}", target.r#match).as_ref());
                    let path_replacement = target.dest.clone();
                    (target.r#type, path_pattern, path_regex, path_replacement)
                })
                .collect::<Vec<_>>();

            for bundle_path in glob(&config.src)
                .expect(&format!("Failed to glob: {}", &config.src))
                .flatten()
            {
                println!("{}", bundle_path.as_path().to_str().unwrap_or_default());

                let mut env = unity_rs::Env::new();
                let data = match std::fs::read(&bundle_path) {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!("Failed to read file: {}\n", e);
                        continue;
                    }
                };
                if !has_supported_asset_bundle_signature(&data) {
                    // this check is necessary because `unity_rs::Env::load_from_slice()` will panic if the file is not a supported asset bundle
                    eprintln!("Unsupported file format\n");
                    continue;
                }
                if env.load_from_slice(&data).is_err() {
                    eprintln!("Failed to load file\n");
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
                            .unwrap_or(String::new()),
                    );
                    placeholder_map.insert("index", index.to_string());
                    placeholder_map.insert(
                        "bundle_path",
                        bundle_path.as_path().to_str().unwrap().to_owned(),
                    );

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
                        let str_path = path.to_str().expect("Failed to convert path to string");

                        println!("  {}", str_path);
                        if !dry_run {
                            std::fs::create_dir_all(path.parent().unwrap())?;
                            dump_asset(str_path, &obj)?;
                        }
                    }
                }
            }
        }
        ArgsAction::Inspect {
            only_supported,
            files,
        } => {
            for file in files {
                println!("{}", file);
                let mut env = unity_rs::Env::new();
                let data = match std::fs::read(file) {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!("  Failed to read file: {}\n", e);
                        continue;
                    }
                };
                if !has_supported_asset_bundle_signature(&data) {
                    // this check is necessary because `unity_rs::Env::load_from_slice()` will panic if the file is not a supported asset bundle
                    eprintln!("  Unsupported file format\n");
                    continue;
                }
                if env.load_from_slice(&data).is_err() {
                    eprintln!("  Failed to load file\n");
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
                            // TODO: use serialized name of ConfigTargetType
                            match r#type {
                                ConfigTargetType::Sprite => "sprite",
                                ConfigTargetType::Texture2D => "texture2d",
                                ConfigTargetType::TextAsset => "text",
                            }
                            .to_owned(),
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
        }
    }

    Ok(())
}
