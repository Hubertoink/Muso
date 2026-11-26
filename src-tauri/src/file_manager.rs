use gtk::gio::{self, prelude::*};
use std::path::Path;

pub fn show_folder(dir: &Path) -> Result<(), String> {
    let uri = gio::File::for_path(dir).uri();
    let service = "org.freedesktop.FileManager1";
    let result = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).and_then(|bus| {
        bus.call_sync(Some(service), "/org/freedesktop/FileManager1", service, "ShowFolders",
            Some(&(vec![uri.as_str()], "").to_variant()), None, gio::DBusCallFlags::NONE,
            5000, gio::Cancellable::NONE)
    });
    if let Err(bus_error) = result {
        gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE)
            .map_err(|e| format!("Could not open {}. File manager: {bus_error}. Default folder handler: {e}", dir.display()))?;
    }
    Ok(())
}
