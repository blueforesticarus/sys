use std::collections::HashSet;

use zbus::Connection;

#[tokio::main]
async fn main() {
    let conn = Connection::system().await.unwrap();
    let manager = zbus_systemd::systemd1::ManagerProxy::new(&conn)
        .await
        .unwrap();

    //dbg!(manager.list_jobs().await.unwrap());
    //dbg!(manager.list_unit_files().await.unwrap());
    let unit_files = manager.list_unit_files().await.unwrap();
    let set: HashSet<_> = unit_files.iter().map(|v| v.1.clone()).collect();
    dbg!(set);
}
