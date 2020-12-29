use std::path::Path;
use std::fs;

pub fn render_directory(relative_path: &str, path: &Path) -> String {
    let mut s = String::new();
    s.push_str("<html><head>");
    s.push_str("</head><body>");
    let paths = fs::read_dir(path).unwrap();
    for path in paths {
        let fname = path.unwrap().file_name();
        let anch = format!("<a href='/{}{}{}'>{}</a>", relative_path, if relative_path.len() > 0 { "/" } else { "" },  fname.to_str().unwrap(), fname.to_str().unwrap());
        s.push_str(&anch);
        s.push_str("</i><br>");
    }
    s.push_str("</body></html>");

    return s;
}
