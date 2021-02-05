use std::fs;
use std::path::Path;

use std::collections::HashMap;
use std::io::Read;

use crate::http::http_core;

const GIT_HASH: &'static str = env!("GIT_HASH");

struct HtmlElement {
    tag: &'static str,
    attributes: Vec<(String, String)>,
    classes: Vec<&'static str>,
    can_have_children: bool,
    children: Option<Vec<HtmlElement>>,
    text: Option<String>,
}

enum HtmlStyle {
    CanHaveChildren, // <element ... > ... </element>
    NoChildren,      // <element ... >
}

impl HtmlElement {
    pub fn new(tag: &'static str, can_have_children: HtmlStyle) -> HtmlElement {
        HtmlElement {
            tag: tag,
            attributes: Vec::new(),
            classes: Vec::new(),
            can_have_children: match can_have_children {
                HtmlStyle::CanHaveChildren => true,
                HtmlStyle::NoChildren => false,
            },
            children: match can_have_children {
                HtmlStyle::CanHaveChildren => Some(Vec::new()),
                HtmlStyle::NoChildren => None,
            },
            text: None,
        }
    }

    pub fn add_text(&mut self, text: String) {
        self.text = Some(text);
    }

    pub fn add_child(&mut self, child: HtmlElement) {
        match &mut self.children {
            Some(children) => {
                children.push(child);
            }
            _ => {}
        };
    }

    pub fn add_attribute(&mut self, key: String, value: String) {
        self.attributes.push((key, value));
    }

    pub fn add_class(&mut self, class: &'static str) {
        self.classes.push(class);
    }

    pub fn render(&self) -> String {
        let attributes = if self.attributes.len() > 0 {
            let mut s = format!("");
            for (attr, val) in &self.attributes {
                s.push_str(&format!(" {}='{}'", attr, val));
            }
            s
        } else {
            format!("")
        };
        let classes = if self.classes.len() > 0 {
            let mut s = format!(" class='");
            for class in &self.classes {
                s.push_str(&format!(" {}", class));
            }
            s.push_str("'");
            s
        } else {
            format!("")
        };
        let mut open_tag = format!("<{}{}{}>", self.tag, attributes, classes);
        if self.can_have_children {
            match &self.text {
                Some(text) => {
                    open_tag.push_str(&text);
                }
                None => {}
            };
            match &self.children {
                Some(children) => {
                    for child in children {
                        open_tag.push_str(&child.render());
                    }
                }
                None => {}
            }
            let close_tag = format!("</{}>", self.tag);
            open_tag.push_str(&close_tag);
        }
        open_tag
    }
}

fn generate_default_footer() -> HtmlElement {
    let mut footer = HtmlElement::new("footer", HtmlStyle::CanHaveChildren);
    let hr = HtmlElement::new("hr", HtmlStyle::NoChildren);
    let mut pre = HtmlElement::new("pre", HtmlStyle::CanHaveChildren);
    pre.add_text(format!("Rendered with hypershare revision {}.", GIT_HASH));

    footer.add_child(hr);
    footer.add_child(pre);
    footer
}

fn generate_href(relative_path: &str, fname: &str) -> String {
    if relative_path.ends_with("/") {
        format!("/{}{}", relative_path, fname)
    } else {
        format!(
            "/{}{}{}",
            relative_path,
            if relative_path.len() > 0 { "/" } else { "" },
            fname
        )
    }
}

fn generate_md5_table(paths: &Vec<std::fs::DirEntry>) -> HashMap<String, String> {
    let mut res = HashMap::<String, String>::new();
    for entry in paths {
        let metadata = match entry.metadata() {
            Ok(meta) => meta,
            _ => {
                continue;
            }
        };
        if !metadata.is_file() {
            continue;
        }
        let is_sum = match entry.path().extension() {
            Some(ext) => ext.to_string_lossy() == "md5sum",
            None => false,
        };
        if !is_sum {
            continue;
        }
        if metadata.len() > 34 {
            continue;
        }
        if let Ok(mut file) = fs::File::open(entry.path()) {
            let mut contents = String::with_capacity(metadata.len() as usize);
            if file.read_to_string(&mut contents).is_ok() {
                if let Some(s) = entry.path().file_name().unwrap().to_str() {
                    res.insert(s.to_string(), contents);
                }
            }
        }
    }
    res
}

fn generate_dir_table(path: &Path, relative_path: &str) -> HtmlElement {
    if let Ok(paths) = fs::read_dir(path) {
        let mut table = HtmlElement::new("table", HtmlStyle::CanHaveChildren);
        let mut paths_vec: Vec<_> = paths.filter_map(Option::Some).map(|r| r.unwrap()).collect();
        paths_vec.sort_by_key(|p| p.path());
        let md5_table = generate_md5_table(&paths_vec);
        for entry in paths_vec {
            let fname = entry.file_name();
            let fname_str = match fname.to_str() {
                Some(f) => f,
                _ => {
                    continue;
                }
            };

            if md5_table.contains_key(fname_str) {
                continue;
            }

            let mut tr = HtmlElement::new("tr", HtmlStyle::CanHaveChildren);

            let meta = match entry.metadata() {
                Ok(m) => m,
                _ => {
                    continue;
                }
            };

            let mut td_type = HtmlElement::new("td", HtmlStyle::CanHaveChildren);
            let mut td_a = HtmlElement::new("td", HtmlStyle::CanHaveChildren);
            let mut td_size = HtmlElement::new("td", HtmlStyle::CanHaveChildren);
            let mut td_hash = HtmlElement::new("td", HtmlStyle::CanHaveChildren);

            // Add pre
            let mut pre_type = HtmlElement::new("pre", HtmlStyle::CanHaveChildren);
            pre_type.add_text(if meta.is_dir() {
                "[DIR]".to_string()
            } else {
                "[FILE]".to_string()
            });
            pre_type.add_attribute(
                "style".to_string(),
                "display: block; text-align: center;".to_string(),
            );
            td_type.add_child(pre_type);

            // Add anchor
            let href = generate_href(relative_path, fname_str);
            let mut a = HtmlElement::new("a", HtmlStyle::CanHaveChildren);
            a.add_attribute("href".to_string(), href);
            a.add_text(fname_str.to_string());
            td_a.add_child(a);

            // Add size
            let mut pre_size = HtmlElement::new("pre", HtmlStyle::CanHaveChildren);
            if meta.is_file() {
                pre_size.add_text(format!("{}", meta.len()));
            }
            pre_size.add_attribute(
                "style".to_string(),
                "display: block; text-align: right;".to_string(),
            );
            td_size.add_child(pre_size);

            match md5_table.get(&format!("{}.md5sum", fname_str)) {
                Some(data) => {
                    let mut pre = HtmlElement::new("pre", HtmlStyle::CanHaveChildren);
                    pre.add_text(format!("MD5: {}", data));
                    td_hash.add_child(pre);
                }
                _ => {}
            }
            tr.add_child(td_type);
            tr.add_child(td_a);
            tr.add_child(td_size);
            tr.add_child(td_hash);

            table.add_child(tr);
        }
        table
    } else {
        let mut p = HtmlElement::new("p", HtmlStyle::CanHaveChildren);
        p.add_text("Error reading directory".to_string());
        p
    }
}

pub fn render_directory(relative_path: &str, path: &Path, show_form: bool) -> String {
    let mut html = HtmlElement::new("html", HtmlStyle::CanHaveChildren);
    let mut head = HtmlElement::new("head", HtmlStyle::CanHaveChildren);
    let mut style = HtmlElement::new("style", HtmlStyle::CanHaveChildren);
    style.add_text(
        r#"
    tr { font-family: monospace; }
    "#
        .to_string(),
    );
    head.add_child(style);
    let mut body = HtmlElement::new("body", HtmlStyle::CanHaveChildren);
    let mut h1 = HtmlElement::new("h1", HtmlStyle::CanHaveChildren);

    // <link rel="shortcut icon" href="data:image/x-icon;," type="image/x-icon">
    let mut link_favi = HtmlElement::new("link", HtmlStyle::NoChildren);
    link_favi.add_attribute("rel".to_string(), "shortcut icon".to_string());
    link_favi.add_attribute("href".to_string(), "data:image/x-icon;,".to_string());
    link_favi.add_attribute("type".to_string(), "image/x-icon".to_string());

    head.add_child(link_favi);
    html.add_child(head);

    h1.add_text(format!("Directory listing for /{}", relative_path));
    body.add_child(h1);
    body.add_child(HtmlElement::new("hr", HtmlStyle::NoChildren));
    let top_level = relative_path.len() == 0;
    if !top_level {
        let mut a = HtmlElement::new("a", HtmlStyle::CanHaveChildren);
        let href = generate_href(relative_path, "..");
        a.add_attribute("href".to_string(), href);
        let mut i = HtmlElement::new("i", HtmlStyle::CanHaveChildren);
        i.add_text("Up a directory".to_string());
        a.add_child(i);
        body.add_child(a);
        body.add_child(HtmlElement::new("br", HtmlStyle::NoChildren));
    }
    let table = generate_dir_table(path, relative_path);
    body.add_child(table);

    if show_form {
        let mut upload_form = HtmlElement::new("form", HtmlStyle::CanHaveChildren);
        upload_form.add_attribute("method".to_string(), "post".to_string());
        upload_form.add_attribute("enctype".to_string(), "multipart/form-data".to_string());
        let mut file_input = HtmlElement::new("input", HtmlStyle::NoChildren);
        file_input.add_attribute("type".to_string(), "file".to_string());
        file_input.add_attribute("name".to_string(), "data".to_string());
        let mut submit_input = HtmlElement::new("input", HtmlStyle::NoChildren);
        submit_input.add_attribute("type".to_string(), "submit".to_string());

        upload_form.add_child(file_input);
        upload_form.add_child(submit_input);

        body.add_child(HtmlElement::new("hr", HtmlStyle::NoChildren));
        body.add_child(upload_form);
    }

    body.add_child(generate_default_footer());
    html.add_child(body);
    html.render()
}

pub fn render_error(status: &http_core::HttpStatus, msg: Option<String>) -> String {
    let mut html = HtmlElement::new("html", HtmlStyle::CanHaveChildren);
    let mut head = HtmlElement::new("head", HtmlStyle::CanHaveChildren);
    let mut body = HtmlElement::new("body", HtmlStyle::CanHaveChildren);
    let mut h1 = HtmlElement::new("h1", HtmlStyle::CanHaveChildren);

    // <link rel="shortcut icon" href="data:image/x-icon;," type="image/x-icon">
    let mut link_favi = HtmlElement::new("link", HtmlStyle::NoChildren);
    link_favi.add_attribute("rel".to_string(), "shortcut icon".to_string());
    link_favi.add_attribute("href".to_string(), "data:image/x-icon;,".to_string());
    link_favi.add_attribute("type".to_string(), "image/x-icon".to_string());
    head.add_child(link_favi);

    h1.add_text(format!(
        "{} {}",
        http_core::status_to_code(status),
        http_core::status_to_message(status)
    ));
    body.add_child(h1);

    body.add_child(HtmlElement::new("hr", HtmlStyle::NoChildren));

    match msg {
        Some(msg) => {
            let mut p = HtmlElement::new("pre", HtmlStyle::CanHaveChildren);
            p.add_text(msg.to_string());
            p.add_class("error");
            body.add_child(p);
        }
        None => {}
    }

    body.add_child(generate_default_footer());
    html.add_child(head);
    html.add_child(body);
    html.render()
}
