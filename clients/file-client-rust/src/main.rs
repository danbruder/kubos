#[macro_use]
extern crate log;
extern crate simplelog;

mod lib;

use lib::file_protocol as protocol;
use simplelog::*;

fn upload(source_path: &str, target_path: &str) {
    info!("Uploading local:{} to remote:{}", source_path, target_path);
    let (hash, num_chunks, mode) = protocol::local_import(source_path).unwrap();
    protocol::send_sync(&hash, num_chunks);
}

fn main() {
    CombinedLogger::init(vec![
        TermLogger::new(LevelFilter::Info, Config::default()).unwrap(),
    ]).unwrap();

    info!("Starting communications service");

    let source = String::from("/home/ryan/Pictures/photoexamples.png");
    let dest = String::from("/home/ryan/test2.txt");

    upload(&source, &dest);
}
