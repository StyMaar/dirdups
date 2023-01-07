use crc32fast::Hasher;
use humanize_rs::bytes::Bytes;
use indicatif::ProgressStyle;
use walkdir::DirEntry;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, prelude::*};
use std::iter;
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use walkdir::WalkDir;

#[derive(StructOpt)]
struct Cli {
    #[structopt(
        short = "m",
        value_name = "N",
        long,
        required = true,
        default_value = "1",
        help = "Ignore files which is smaller than this size"
    )]
    min_size: String,

    #[structopt(
        short = "i",
        value_name = "N",
        long,
        required = true,
        default_value = "10",
        help = "How many equal files must be in 2 directories to consider those directories as duplicates"
    )]
    min_intersection: usize,

    #[structopt(
        short = "h",
        value_name = "N",
        long,
        required = true,
        default_value = "1024",
        help = "Reads only N bytes to calculate checksum. Set 0 to read full file."
    )]
    head: String,

    #[structopt(long, required = true, index = 1, help = "Directory to search")]
    directories: PathBuf,
}

struct Duplicate {
    dir1: PathBuf,
    dir2: PathBuf,
    dir1_files_number: usize,
    dir2_files_number: usize,
    intersection: usize,
}

fn get_hash(path: impl AsRef<Path>, filesize: usize, read_first_bytes: usize) -> io::Result<u64> {
    let crc32 = get_crc32_checksum(path, read_first_bytes)?;
    Ok(crc32 as u64 + filesize as u64)
}

fn get_crc32_checksum(path: impl AsRef<Path>, read_first_bytes: usize) -> io::Result<u32> {
    let mut f = File::open(path)?;
    let mut hasher = Hasher::new();
    const BUF_SIZE: usize = 1024;
    let mut buffer: [u8; BUF_SIZE] = [0; BUF_SIZE];

    let mut bytes_readed = 0;
    loop {
        if read_first_bytes > 0 && bytes_readed >= read_first_bytes {
            break;
        }
        let n = f.read(&mut buffer[..])?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[0..n]);
        bytes_readed += n;
    }
    Ok(hasher.finalize())
}

fn get_files(dir_path: impl AsRef<Path>) -> impl Iterator<Item= DirEntry> {
    
    let iter = walk_dir(dir_path).chain(iter::empty());

    iter
}

fn walk_dir(path: impl AsRef<Path>)-> impl Iterator<Item= DirEntry>{
    WalkDir::new(path)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
}

fn load_files_info(
    files: impl Iterator<Item=DirEntry>,
    min_size: usize,
    head: usize,
    hash_dirs: &mut HashMap<u64, HashSet<PathBuf>>,
    dir_hashes: &mut HashMap<PathBuf, HashSet<u64>>,
) {

    let pb = indicatif::ProgressBar::new_spinner();
        
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} [{elapsed_precise}]"));
    for file in files {
        pb.inc(1);
        let filesize = file.metadata().unwrap().len() as usize;
        if filesize < min_size {
            continue;
        }

        let dir = file.path().parent().unwrap().to_owned();
        
        let hash = get_hash(file.path(), filesize, head).unwrap();

        if let Some(val) = hash_dirs.get_mut(&hash) {
            val.insert(dir.clone());
        } else {
            let mut dirs = HashSet::new();
            dirs.insert(dir.clone());
            hash_dirs.insert(hash.clone(), dirs);
        }

        if let Some(val) = dir_hashes.get_mut(&dir) {
            val.insert(hash.clone());
        } else {
            let mut hashes = HashSet::new();
            hashes.insert(hash.clone());
            dir_hashes.insert(dir, hashes);
        }
    }
}

fn find_duplicates(
    hash_dirs: &HashMap<u64, HashSet<PathBuf>>,
    dir_hashes: &HashMap<PathBuf, HashSet<u64>>,
) -> Vec<Duplicate> {
    let mut duplicates = Vec::new();
    let mut added = HashSet::new();

    for (_, dirs) in hash_dirs.iter() {
        let mut dirs_iter = dirs.iter();
        let mut prev_dir = match dirs_iter.next() {
            Some(v) => v,
            None => break,
        };

        for dir in dirs_iter {
            let t = if *dir < *prev_dir {
                (dir, prev_dir)
            } else {
                (prev_dir, dir)
            };
            if added.contains(&t) {
                continue;
            }
            let files = dir_hashes.get(dir).unwrap();
            let prev_files = dir_hashes.get(prev_dir).unwrap();
            let intersection: HashSet<_> = files.intersection(&prev_files).collect();
            let duplicate = Duplicate {
                dir1: dir.to_owned(),
                dir2: prev_dir.to_owned(),
                dir1_files_number: files.len(),
                dir2_files_number: prev_files.len(),
                intersection: intersection.len(),
            };
            duplicates.push(duplicate);

            added.insert(t);
            prev_dir = dir;
        }
    }
    duplicates
}

fn print_duplicates(duplicates: &Vec<Duplicate>) {
    for duplicate in duplicates.iter() {
        println!(
            "{}: {} - {}: {} | {}",
            duplicate.dir1.to_string_lossy(),
            duplicate.dir1_files_number,
            duplicate.dir2.to_string_lossy(),
            duplicate.dir2_files_number,
            duplicate.intersection
        )
    }
}

fn main() {
    let args = Cli::from_args();

    let min_size = match args.min_size.parse::<Bytes>() {
        Ok(some) => some.size(),
        Err(_) => {
            eprintln!("Invalid value for '--min-size': {}.", args.min_size);
            return;
        }
    };

    let mut head = match args.head.parse::<Bytes>() {
        Ok(some) => some.size(),
        Err(_) => {
            eprintln!("Invalid value for '--head': {}.", args.min_size);
            return;
        }
    };
    if head > 0 && head < 1000 {
        head = 1024;
        eprintln!(
            "Warning!: --min-size values >0 and <1000 are ignored. Default value of 1024 is used."
        );
    }

    let mut hash_dirs: HashMap<u64, HashSet<PathBuf>> = HashMap::new();
    let mut dir_hashes: HashMap<PathBuf, HashSet<u64>> = HashMap::new();

    let files = get_files(args.directories);
    load_files_info(files, min_size, head, &mut hash_dirs, &mut dir_hashes);

    let mut duplicates: Vec<Duplicate> = find_duplicates(&hash_dirs, &dir_hashes)
        .into_iter()
        .filter(|x| x.intersection >= args.min_intersection)
        .collect();

    duplicates.sort_by_key(|x| Reverse(x.intersection));

    print_duplicates(&duplicates);
}
