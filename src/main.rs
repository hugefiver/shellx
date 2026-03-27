use relm4::RelmApp;
use shellx::app::ShellXApp;

fn main() {
    shellx::theme::init_logging();
    shellx::theme::register_resources();
    RelmApp::new("io.github.hugefiver.shellx").run::<ShellXApp>(());
}
