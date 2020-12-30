use std::path::Path;
use std::fs;

use crate::server::simple_http;

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
            children: if can_have_children { Some(Vec::new()) } else { None },
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
            },
            _ => {},
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
                Some(text) => { open_tag.push_str(&text); },
                None => {}
            };
            match &self.children {
                Some(children) => {
                    for child in children {
                        open_tag.push_str(&child.render());
                    }
                },
                None => {},
            }
            let close_tag = format!("</{}>", self.tag);
            open_tag.push_str(&close_tag);
        }
        open_tag
    }
}


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

pub fn render_error(status: &simple_http::HttpStatus, msg: Option<&str>) -> String {
    let mut html = HtmlElement::new("html", true);
    let mut body = HtmlElement::new("body", true);
    let mut h1 = HtmlElement::new("h1", true);
    h1.add_text(format!("{} {}", simple_http::status_to_code(status), simple_http::status_to_message(status)));
    body.add_child(h1);

    match msg {
        Some(msg) => {
            let mut p = HtmlElement::new("p", true);
            p.add_text(msg.to_string());
            p.add_class("error");
            body.add_child(p);
        }
        None => {},
    }
    html.add_child(body);
    html.render()
}
