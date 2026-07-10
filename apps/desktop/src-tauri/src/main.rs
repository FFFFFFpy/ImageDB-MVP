fn main() {
    if std::env::args().any(|arg| arg == "--imagedb-install-gate-managed-bootstrap") {
        imagedb_desktop_lib::enable_install_gate_managed_probe();
    }
    if std::env::args().any(|arg| arg == "--imagedb-install-gate-launch-smoke") {
        imagedb_desktop_lib::enable_install_gate_launch_smoke();
    }
    imagedb_desktop_lib::run();
}
