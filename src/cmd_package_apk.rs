use crate::apt_repo::fetch_repo;
use crate::deb_file::{visit_files, DebVisitor};
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::fs::File;
use std::io::{copy, Error, ErrorKind, Read, Write};
use std::sync::{Arc, RwLock};
use std::thread;

pub struct CreateApkVisitor {
    output_directory: String,
    counter: u32,
    file_mapping: String,
    symlinks: String,
}

impl DebVisitor for CreateApkVisitor {
    fn visit_control(&mut self, _: HashMap<String, String, RandomState>) {
        // Do nothing.
    }

    fn visit_file(&mut self, file: &mut tar::Entry<impl Read>) {
        //let file_path_full: String;
        let header = file.header();
        let is_symlink = header.entry_type() == tar::EntryType::Symlink;
        let is_regular = header.entry_type() == tar::EntryType::Regular;
        if !(is_regular || is_symlink) {
            return;
        }

        let pp = file.path().unwrap();
        let file_path = pp.to_str().unwrap();
        let relative_path = &file_path[33..];
        //file_path_full = String::from(&file_path[2..]);
        if is_symlink {
            if !self.symlinks.is_empty() {
                self.symlinks = format!("{}\n", self.symlinks);
            }
            self.symlinks = format!(
                "{}{}←{}",
                self.symlinks,
                header.link_name().unwrap().unwrap().to_str().unwrap(),
                relative_path
            );
        } else {
            if !self.file_mapping.is_empty() {
                self.file_mapping = format!("{}\n", self.file_mapping);
            }
            self.file_mapping =
                format!("{}{}.so←{}", self.file_mapping, self.counter, relative_path);

            let file_path = format!("{}/{}.so", self.output_directory, self.counter);
            let mut output = File::create(file_path).unwrap();
            copy(file, &mut output).unwrap();
            self.counter += 1;
        }
    }
}

fn write_bytes_to_file(path: &str, file_content: &[u8]) -> Result<(), Error> {
    let mut output = File::create(path)?;
    output.write_all(file_content)?;
    Ok(())
}

fn write_string_to_file(path: &str, file_content: &str) -> Result<(), Error> {
    let mut output = File::create(path)?;
    write!(output, "{}", file_content)?;
    Ok(())
}

fn create_dir(path: &str) {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) => {
            if error.kind() == ErrorKind::AlreadyExists {
                eprintln!("Output directory already exists: {}", path);
                std::process::exit(1);
            } else {
                panic!("{}", error.to_string());
            }
        }
    }
}

pub fn create_apk(package_name: &str, output_dir: &str) {
    create_dir(output_dir);
    create_dir(&format!("{}/app", output_dir));
    create_dir(&format!("{}/app/src", output_dir));
    create_dir(&format!("{}/app/src/main", output_dir));
    create_dir(&format!("{}/app/src/main/jniLibs", output_dir));

    let android_manifest = include_str!("AndroidManifest.xml");
    let android_manifest = android_manifest.replace("PACKAGE_NAME", package_name);

    write_string_to_file(
        &format!("{}/build.gradle", output_dir),
        include_str!("build.gradle"),
    )
    .unwrap();
    write_string_to_file(&format!("{}/settings.gradle", output_dir), "include ':app'").unwrap();
    write_bytes_to_file(
        &format!("{}/app/dev_keystore.jks", output_dir),
        include_bytes!("dev_keystore.jks"),
    )
    .unwrap();
    write_string_to_file(
        &format!("{}/app/build.gradle", output_dir),
        &include_str!("app-build.gradle").replace("PACKAGE_NAME", package_name),
    )
    .unwrap();
    write_string_to_file(
        &format!("{}/app/src/main/AndroidManifest.xml", output_dir),
        &android_manifest,
    )
    .unwrap();

    let arch_all_packages = fetch_repo("all");
    let arch_all_packages = Arc::new(RwLock::new(arch_all_packages));

    let mut join_handles = Vec::new();
    for arch in &["arm", "aarch64", "i686", "x86_64"] {
        // x86', 'x86_64', 'armeabi-v7a', 'arm64-v8a
        let android_abi_name = match *arch {
            "arm" => "armeabi-v7a",
            "aarch64" => "arm64-v8a",
            "i686" => "x86",
            "x86_64" => "x86_64",
            _ => {
                panic!();
            }
        };
        create_dir(&format!(
            "{}/app/src/main/jniLibs/{}",
            output_dir, android_abi_name
        ));
        let my_arch_all_packages = Arc::clone(&arch_all_packages);

        let output_dir = output_dir.to_string();
        let package_name = package_name.to_string();
        join_handles.push(thread::spawn(move || {
            let http_client = reqwest::blocking::Client::new();
            let packages = fetch_repo(arch);
            let arch_all = my_arch_all_packages.read().unwrap();
            let bootstrap_package = packages
                .get(&package_name)
                .or_else(|| arch_all.get(&package_name))
                .unwrap_or_else(|| panic!("Cannot find package '{}'", package_name));
            let package_url = bootstrap_package.package_url();

            let mut response = http_client
                .get(&package_url)
                .send()
                .unwrap_or_else(|_| panic!("Failed fetching {}", package_url));
            let mut visitor = CreateApkVisitor {
                output_directory: format!(
                    "{}/app/src/main/jniLibs/{}",
                    output_dir, android_abi_name
                ),
                counter: 0,
                file_mapping: String::new(),
                symlinks: String::new(),
            };
            visit_files(&mut response, &mut visitor);

            write_string_to_file(
                &format!(
                    "{}/app/src/main/jniLibs/{}/files.so",
                    output_dir, android_abi_name
                ),
                &visitor.file_mapping,
            )
            .unwrap();
            write_string_to_file(
                &format!(
                    "{}/app/src/main/jniLibs/{}/symlinks.so",
                    output_dir, android_abi_name
                ),
                &visitor.symlinks,
            )
            .unwrap();
        }));
    }
    for handle in join_handles {
        handle.join().unwrap();
    }
}