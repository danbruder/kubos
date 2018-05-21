#[macro_use]
extern crate log;
extern crate file_protocol;
extern crate simplelog;

use simplelog::*;

use file_protocol::CborProtocol;
use file_protocol::FileProtocol;

// fn upload(source_path: &str, target_path: &str) {
//     info!("Uploading local:{} to remote:{}", source_path, target_path);
//     let (hash, num_chunks, mode) = protocol::local_import(source_path).unwrap();
//     protocol::send_sync(&hash, num_chunks);
// }

fn main() {
    CombinedLogger::init(vec![
        TermLogger::new(LevelFilter::Info, Config::default()).unwrap(),
    ]).unwrap();

    info!("Starting communications service");

    let source_path = String::from("/home/ryan/Pictures/photoexamples.png");
    let target_path = String::from("/home/ryan/test2.txt");

    let c_protocol = CborProtocol::new(0);

    let f_protocol = FileProtocol {
        cbor_proto: c_protocol,
        host: String::from("127.0.0.1"),
        dest_port: 7000,
    };
    info!(
        "Uploading local:{} to remote:{}",
        &source_path, &target_path
    );
    let (hash, num_chunks, mode) = f_protocol.local_import(&source_path).unwrap();
    f_protocol.send_sync(&hash, num_chunks).unwrap();
    f_protocol.send_export(&hash, &target_path, mode);
}
