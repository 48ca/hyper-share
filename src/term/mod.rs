use nix::sys::termios;

pub fn check_terminal() -> bool {
    let stdout_fd = 1;
    if let Ok(_attrs) = termios::tcgetattr(stdout_fd) {
        // Potentially do some validation in the future.
        return true;
    }
    return false;
}
