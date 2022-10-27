use std::{
    collections::HashSet,
    path::PathBuf,
    str::FromStr,
};

use zbus::Connection;
use zbus_systemd::systemd1::UnitProxy;

#[tokio::main]
async fn main() {
    let conn = Connection::session().await.unwrap();
    let manager = zbus_systemd::systemd1::ManagerProxy::new(&conn)
        .await
        .unwrap();

    //dbg!(manager.list_jobs().await.unwrap());
    //dbg!(manager.list_unit_files().await.unwrap());
    let unit_files = manager.list_unit_files().await.unwrap();
    let set: HashSet<_> = unit_files.iter().map(|v| v.1.clone()).collect();
    dbg!(set);

    for (unit, state) in unit_files {
        if state == "disabled" {
            println!("{unit} {state}");
            let name = PathBuf::from_str(&unit).unwrap();
            let name = name.file_name().unwrap().to_str().unwrap();

            match manager.load_unit(name.to_string()).await {
                Err(e) => println!("{e}"),
                Ok(v) => {
                    let p = UnitProxy::builder(&conn)
                        .path(v)
                        .unwrap()
                        .build()
                        .await
                        .unwrap();
                }
            }
        }
    }
    let v: Vec<_> = manager
        .list_units()
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.0)
        .collect();

    dbg!(v);
}
