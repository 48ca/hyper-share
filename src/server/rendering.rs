use std::path::Path;
use std::fs;

pub fn render_directory(path: &Path) -> String {
    let mut s = String::new();
    s.push_str("<html><body>");
    let paths = fs::read_dir(path).unwrap();
    for path in paths {
        s.push_str("<a href>");
        s.push_str(path.unwrap().file_name().to_str().unwrap());
        s.push_str("</i><br>");
    }
    s.push_str("</body></html>");

    return s;
}
