use std::fs;
use std::path::Path;

use crate::server::http;

const GIT_HASH: &'static str = env!("GIT_HASH");

struct HtmlElement {
    tag: &'static str,
    attributes: Vec<(String, String)>,
    classes: Vec<&'static str>,
    can_have_children: bool,
    children: Option<Vec<HtmlElement>>,
    text: Option<String>,
}

impl HtmlElement {
    pub fn new(tag: &'static str, can_have_children: bool) -> HtmlElement {
        HtmlElement {
            tag: tag,
            attributes: Vec::new(),
            classes: Vec::new(),
            can_have_children: can_have_children,
            children: if can_have_children {
                Some(Vec::new())
            } else {
                None
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
            let mut s = format!(" ");
            for (attr, val) in &self.attributes {
                s.push_str(&format!("{}='{}'", attr, val));
            }
            s
        } else {
            format!("")
        };
        let classes = if self.classes.len() > 0 {
            let mut s = format!("class='");
            for class in &self.classes {
                s.push_str(class);
            }
            s.push_str("'");
            s
        } else {
            format!("")
        };
        let mut open_tag = format!("<{} {} {}>", self.tag, attributes, classes);
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
    let mut footer = HtmlElement::new("footer", true);
    let hr = HtmlElement::new("hr", false);
    let mut pre = HtmlElement::new("pre", true);
    pre.add_text(format!("Rendered with httptui revision {}.", GIT_HASH));

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

pub fn render_directory(relative_path: &str, path: &Path) -> String {
    let mut html = HtmlElement::new("html", true);
    let mut body = HtmlElement::new("body", true);
    let mut h1 = HtmlElement::new("h1", true);
    h1.add_text(format!("Directory listing for /{}", relative_path));
    body.add_child(h1);
    body.add_child(HtmlElement::new("hr", false));
    let top_level = relative_path.len() == 0;
    if !top_level {
        let mut a = HtmlElement::new("a", true);
        let href = generate_href(relative_path, "..");
        a.add_attribute("href".to_string(), href);
        let mut i = HtmlElement::new("i", true);
        i.add_text("Up a directory".to_string());
        a.add_child(i);
        body.add_child(a);
        body.add_child(HtmlElement::new("br", false));
    }
    if let Ok(paths) = fs::read_dir(path) {
        let mut table = HtmlElement::new("table", true);
        for path in paths {
            let entry = match path {
                Ok(p) => p,
                _ => {
                    continue;
                }
            };
            let fname = entry.file_name();
            let fname_str = match fname.to_str() {
                Some(f) => f,
                _ => {
                    continue;
                }
            };
            let mut tr = HtmlElement::new("tr", true);

            let meta = match entry.metadata() {
                Ok(m) => m,
                _ => {
                    continue;
                }
            };

            let mut td_type = HtmlElement::new("td", true);
            let mut td_a = HtmlElement::new("td", true);
            let mut td_size = HtmlElement::new("td", true);

            // Add pre
            let mut pre_type = HtmlElement::new("pre", true);
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
            let mut a = HtmlElement::new("a", true);
            a.add_attribute("href".to_string(), href);
            a.add_text(fname_str.to_string());
            td_a.add_child(a);

            // Add size
            let mut pre_size = HtmlElement::new("pre", true);
            if meta.is_file() {
                pre_size.add_text(format!("{}", meta.len()));
            }
            pre_size.add_attribute(
                "style".to_string(),
                "display: block; text-align: right;".to_string(),
            );
            td_size.add_child(pre_size);

            tr.add_child(td_type);
            tr.add_child(td_a);
            tr.add_child(td_size);

            table.add_child(tr);
        }
        body.add_child(table);
        body.add_child(generate_default_footer());
        html.add_child(body);
        html.render()
    } else {
        "Error reading directory".to_string()
    }
}

pub fn render_error(status: &http::HttpStatus, msg: Option<&str>) -> String {
    let mut html = HtmlElement::new("html", true);
    let mut body = HtmlElement::new("body", true);
    let mut h1 = HtmlElement::new("h1", true);

    h1.add_text(format!(
        "{} {}",
        http::status_to_code(status),
        http::status_to_message(status)
    ));
    body.add_child(h1);

    body.add_child(HtmlElement::new("hr", false));

    match msg {
        Some(msg) => {
            let mut p = HtmlElement::new("pre", true);
            p.add_text(msg.to_string());
            p.add_class("error");
            body.add_child(p);
        }
        None => {}
    }

    body.add_child(generate_default_footer());
    html.add_child(body);
    html.render()
}
