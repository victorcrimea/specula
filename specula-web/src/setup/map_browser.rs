//! The setup's map browser (`gui/maps/mapbrowser/`, `MapBrowserPage.js`):
//! a grid of map previews with a search, a type and a filter of its own;
//! Select sends the map, its type and filter as one change (`BrowseMap`).
//! The grid scrolls where vanilla pages it.

use std::collections::HashMap;
use std::rc::Rc;

use js_sys::Array;
use js_sys::JsString;
use js_sys::Math;
use js_sys::Object;
use js_sys::Reflect;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::Document;
use web_sys::Element;
use web_sys::Event;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;
use web_sys::HtmlInputElement;
use web_sys::HtmlSelectElement;

use super::Page;
use super::Shared;
use super::View;
use super::by_id;
use super::clear;
use super::create;
use super::enable;
use super::fill;
use super::get;
use super::list;
use super::listen;
use super::object;
use super::option;
use super::preview_stem;
use super::send;
use super::show;
use super::strings;
use super::text;
use super::title_case;
use crate::lang;
use crate::lang::translate;
use crate::lang::translate_context as tc;

/// `MapGridBrowser.prototype.ItemRatio`.
const ITEM_RATIO: f64 = 4.0 / 3.0;
const DEFAULT_ITEM_WIDTH: f64 = 200.0;
const MIN_ITEM_WIDTH: f64 = 100.0;
/// SDL's name of vanilla's `cancel` hotkey (`config/default.cfg`).
const CANCEL_KEY: &str = "Escape";
/// `MatchSort.Highscore`.
const HIGHSCORE: i64 = -100_000_000;

/// One entry of `MapGridBrowser.mapList`.
#[derive(Clone)]
struct Entry {
    /// The map path, or "random".
    file: String,
    name: String,
    description: String,
    /// The type that listed it.
    map_type: String,
    /// The filter that listed it.
    filter: String,
    /// The stem for `--gui-mappreview-<stem>`.
    preview: String,
}

pub(super) struct MapBrowser {
    document: Document,
    root: HtmlElement,
    grid: HtmlElement,
    count: HtmlElement,
    search: HtmlInputElement,
    map_type: HtmlSelectElement,
    map_filter: HtmlSelectElement,
    zoom_in: HtmlButtonElement,
    zoom_out: HtmlButtonElement,
    selected_preview: HtmlElement,
    selected_name: HtmlElement,
    selected_description: HtmlElement,
    pick_random: HtmlButtonElement,
    cancel: HtmlButtonElement,
    select: HtmlButtonElement,
    open: bool,
    /// `allowSelection`: the viewer hosts.
    allow_selection: bool,
    list: Vec<Entry>,
    /// Index into `list`.
    selected: Option<usize>,
    tiles: Vec<HtmlElement>,
    /// `columnCount`, kept until the page reloads.
    columns: Option<u32>,
    /// Preview URLs read from session.css, by stem.
    urls: HashMap<String, String>,
}

impl MapBrowser {
    pub(super) fn new(document: &Document) -> Result<MapBrowser, String> {
        Ok(MapBrowser {
            document: document.clone(),
            root: by_id(document, "map-browser")?,
            grid: by_id(document, "map-grid")?,
            count: by_id(document, "map-count")?,
            search: by_id(document, "map-search")?,
            map_type: by_id(document, "map-browser-type")?,
            map_filter: by_id(document, "map-browser-filter")?,
            zoom_in: by_id(document, "map-zoom-in")?,
            zoom_out: by_id(document, "map-zoom-out")?,
            selected_preview: by_id(document, "map-selected-preview")?,
            selected_name: by_id(document, "map-selected-name")?,
            selected_description: by_id(document, "map-selected-description")?,
            pick_random: by_id(document, "map-pick-random")?,
            cancel: by_id(document, "map-cancel")?,
            select: by_id(document, "map-select")?,
            open: false,
            allow_selection: false,
            list: Vec::new(),
            selected: None,
            tiles: Vec::new(),
            columns: None,
            urls: HashMap::new(),
        })
    }

    /// `MapBrowserPageControls.onOpenPage`: what a host and a viewer get.
    fn render_mode(&self) {
        let allow = self.allow_selection;
        show(&self.pick_random, allow);
        show(&self.select, allow);
        let hotkey = format!("[{CANCEL_KEY}]");
        let (caption, tip) = if allow {
            (
                translate("Cancel"),
                translate("%(hotkey)s: Close map browser and discard the selection."),
            )
        } else {
            (
                translate("Close"),
                translate("%(hotkey)s: Close map browser."),
            )
        };
        self.cancel.set_text_content(Some(&caption));
        let _ = self
            .cancel
            .set_attribute("title", &lang::format(&tip, &[("hotkey", &hotkey)]));
    }

    /// `MapFilters.getAvailableMapFilters`.
    fn available_filters(catalog: &JsValue, map_type: &str) -> Vec<String> {
        let maps = list(catalog, "maps");
        strings_of(catalog)
            .into_iter()
            .filter(|name| {
                maps.iter().any(|map| {
                    text(map, "kind") == map_type && strings(map, "filters").contains(name)
                })
            })
            .collect()
    }

    /// `MapFiltering.renderMapFilter`.
    fn render_filter_options(&self, catalog: &JsValue) {
        let titles = list(catalog, "map_filters");
        let available = Self::available_filters(catalog, &self.map_type.value());
        let current = self.map_filter.value();
        let keep = if available.contains(&current) {
            current
        } else {
            available.first().cloned().unwrap_or_default()
        };
        let options: Vec<(String, String)> = available
            .iter()
            .map(|name| {
                let title = titles
                    .iter()
                    .find(|filter| text(filter, "name") == *name)
                    .map(|filter| text(filter, "title"))
                    .unwrap_or_default();
                option(name, translate(&title))
            })
            .collect();
        fill(&self.document, &self.map_filter, &options, &keep);
    }

    /// `MapFiltering.onChange`.
    fn on_change(&mut self, catalog: &JsValue) {
        self.update_map_list(catalog);
        self.scroll_to_selected();
    }

    /// `MapGridBrowser.updateMapList`.
    fn update_map_list(&mut self, catalog: &JsValue) {
        let selected = self.selected.map(|index| self.list[index].file.clone());
        let map_type = self.map_type.value();
        let filter = self.map_filter.value();
        let query = self.search.value().trim().to_string();
        let mut entries = filtered_maps(catalog, &map_type, &filter);
        if map_type == "random" {
            entries.insert(0, random_entry());
        }
        if !query.is_empty() {
            entries = match_sort(&query, entries);
            if entries.is_empty() {
                entries.push(random_entry());
                for kind in strings(catalog, "map_types") {
                    entries.extend(filtered_maps(catalog, &kind, "all"));
                }
                entries = match_sort(&query, entries);
            }
        }
        self.list = entries;
        self.selected = None;
        self.render_grid();
        let found = selected.and_then(|file| self.list.iter().position(|e| e.file == file));
        self.set_selected(found);
        self.count.set_text_content(Some(&lang::format(
            &translate("Maps: %(mapCount)s"),
            &[("mapCount", &self.list.len().to_string())],
        )));
        self.resize_grid();
    }

    /// `MapGridBrowserItem.updateMapAssignment`.
    fn render_grid(&mut self) {
        clear(&self.grid);
        self.tiles.clear();
        for index in 0..self.list.len() {
            let entry = self.list[index].clone();
            let url = self.preview_url(&entry.preview);
            let tile: HtmlElement = create(&self.document, "button");
            let _ = tile.set_attribute("type", "button");
            tile.set_class_name("map-tile");
            let _ = tile.set_attribute("data-index", &index.to_string());
            let frame: HtmlElement = create(&self.document, "span");
            frame.set_class_name("frame gold-box");
            if !url.is_empty() {
                let image: HtmlElement = create(&self.document, "img");
                let _ = image.set_attribute("loading", "lazy");
                let _ = image.set_attribute("decoding", "async");
                let _ = image.set_attribute("alt", "");
                let _ = image.set_attribute("src", &url);
                let _ = frame.append_child(&image);
            }
            let name: HtmlElement = create(&self.document, "span");
            name.set_class_name("name");
            name.set_text_content(Some(&entry.name));
            let _ = frame.append_child(&name);
            let _ = tile.append_child(&frame);
            let _ = self.grid.append_child(&tile);
            self.tiles.push(tile);
        }
    }

    /// The URL `session.css` gives the preview of `stem`.
    fn preview_url(&mut self, stem: &str) -> String {
        if let Some(url) = self.urls.get(stem) {
            return url.clone();
        }
        let read = |stem: &str| -> String {
            let Some(element) = self.document.document_element() else {
                return String::new();
            };
            let Some(window) = web_sys::window() else {
                return String::new();
            };
            let Ok(Some(style)) = window.get_computed_style(&element) else {
                return String::new();
            };
            style
                .get_property_value(&format!("--gui-mappreview-{stem}"))
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let mut value = read(stem);
        if value.is_empty() {
            value = read("nopreview");
        }
        let url = value
            .strip_prefix("url(")
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or("")
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_string();
        self.urls.insert(stem.to_string(), url.clone());
        url
    }

    /// `GridBrowser.setSelectedIndex` and its handlers.
    fn set_selected(&mut self, index: Option<usize>) {
        if let Some(old) = self.selected.and_then(|old| self.tiles.get(old)) {
            old.set_class_name("map-tile");
            let _ = old.set_attribute("aria-pressed", "false");
        }
        self.selected = index.filter(|index| *index < self.list.len());
        if let Some(tile) = self.selected.and_then(|new| self.tiles.get(new)) {
            tile.set_class_name("map-tile selected");
            let _ = tile.set_attribute("aria-pressed", "true");
        }
        enable(&self.select, self.selected.is_some());
        self.render_selection();
    }

    /// `MapDescription.onSelectionChange`.
    fn render_selection(&self) {
        let Some(entry) = self.selected.map(|index| &self.list[index]) else {
            return;
        };
        self.selected_name.set_text_content(Some(&entry.name));
        self.selected_description
            .set_text_content(Some(&entry.description));
        let _ = self.selected_preview.style().set_property(
            "background-image",
            &format!(
                "var(--gui-mappreview-{}, var(--gui-mappreview-nopreview))",
                entry.preview
            ),
        );
    }

    /// `GridBrowser.resizeGrid`.
    fn resize_grid(&mut self) {
        let width = f64::from(self.grid.client_width());
        let height = f64::from(self.grid.client_height());
        if width <= 0.0 {
            return;
        }
        let max = ((width / MIN_ITEM_WIDTH).floor() as u32).max(1);
        let min = ((width / (height.max(1.0) * ITEM_RATIO)).ceil() as u32).clamp(1, max);
        let columns = self
            .columns
            .unwrap_or((width / DEFAULT_ITEM_WIDTH).floor() as u32)
            .clamp(min, max);
        self.columns = Some(columns);
        let _ = self
            .grid
            .style()
            .set_property("--columns", &columns.to_string());
        enable(&self.zoom_in, columns > min);
        enable(&self.zoom_out, columns < max);
    }

    /// `GridBrowser.increaseColumnCount`.
    fn zoom(&mut self, diff: i32) {
        let visible = self
            .selected
            .and_then(|index| self.tiles.get(index))
            .is_some_and(|tile| {
                let top = tile.offset_top();
                top >= self.grid.scroll_top()
                    && top + tile.offset_height()
                        <= self.grid.scroll_top() + self.grid.client_height()
            });
        let Some(columns) = self.columns else {
            return;
        };
        self.columns = Some(columns.saturating_add_signed(diff).max(1));
        self.resize_grid();
        if visible {
            self.scroll_to_selected();
        }
    }

    /// `GridBrowser.goToPageOfSelected`: centres the selected row.
    fn scroll_to_selected(&self) {
        let top = self
            .selected
            .and_then(|index| self.tiles.get(index))
            .map_or(0, |tile| {
                (tile.offset_top() - (self.grid.client_height() - tile.offset_height()) / 2).max(0)
            });
        self.grid.set_scroll_top(top);
    }

    /// The tile index under an event's target.
    fn tile_index(event: &Event) -> Option<usize> {
        event
            .target()
            .and_then(|target| target.dyn_into::<Element>().ok())
            .and_then(|element| element.closest(".map-tile").ok().flatten())
            .and_then(|tile| tile.get_attribute("data-index"))
            .and_then(|index| index.parse().ok())
    }
}

/// The names of `catalog.map_filters`, in order.
fn strings_of(catalog: &JsValue) -> Vec<String> {
    list(catalog, "map_filters")
        .iter()
        .map(|filter| text(filter, "name"))
        .collect()
}

/// `MapFilters.getFilteredMaps`, sorted as the server's `map_list` is.
fn filtered_maps(catalog: &JsValue, map_type: &str, filter: &str) -> Vec<Entry> {
    if map_type.is_empty() || filter.is_empty() {
        return Vec::new();
    }
    let mut maps: Vec<JsValue> = list(catalog, "maps")
        .into_iter()
        .filter(|map| {
            text(map, "kind") == map_type && strings(map, "filters").iter().any(|f| f == filter)
        })
        .collect();
    maps.sort_by_key(|map| text(map, "name").to_lowercase());
    maps.iter()
        .map(|map| Entry {
            file: text(map, "path"),
            name: translate(&text(map, "name")),
            description: translate(&text(map, "description")),
            map_type: map_type.to_string(),
            filter: filter.to_string(),
            preview: preview_stem(&text(map, "preview")),
        })
        .collect()
}

/// The `randomMap` of `updateMapList`.
fn random_entry() -> Entry {
    Entry {
        file: "random".to_string(),
        name: tc("map selection", "Random"),
        description: translate("Pick a map at random."),
        map_type: "random".to_string(),
        filter: "default".to_string(),
        preview: "nopreview".to_string(),
    }
}

/// `MatchSort.get`.
fn match_sort(input: &str, entries: Vec<Entry>) -> Vec<Entry> {
    let input = input.to_lowercase();
    let mut scored: Vec<(Entry, i64, bool)> = entries
        .into_iter()
        .filter_map(|entry| {
            let score = score_text(&input, &entry.name)?;
            let starts = entry.name.starts_with(&input);
            Some((entry, score, starts))
        })
        .collect();
    scored.sort_by(
        |(a, score_a, starts_a), (b, score_b, starts_b)| match (starts_a, starts_b) {
            (true, true) => JsString::from(a.name.as_str())
                .locale_compare(&b.name, &Array::new(), &Object::new())
                .cmp(&0),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => score_a.cmp(score_b),
        },
    );
    scored.into_iter().map(|(entry, _, _)| entry).collect()
}

/// `MatchSort.scoreText`; the lower the better.
fn score_text(input: &str, name: &str) -> Option<i64> {
    if input == name {
        return Some(HIGHSCORE);
    }
    let name: Vec<char> = name.to_lowercase().chars().collect();
    if input.chars().eq(name.iter().copied()) {
        return Some(HIGHSCORE / 2);
    }
    let mut score = 0_i64;
    let mut offset: i64 = -1;
    for letter in input.chars() {
        let from = usize::try_from(offset + 1).unwrap_or(0);
        let next = name.get(from..)?.iter().position(|c| *c == letter)? as i64 + from as i64;
        let consecutive = i64::from(next != offset + 1);
        score += next + consecutive * next;
        offset = next;
    }
    Some(score)
}

/// Whether the main pointer is precise, as a mouse; a touch screen's
/// keyboard would cover the grid.
fn fine_pointer() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Ok(function) = Reflect::get(&window, &"matchMedia".into()) else {
        return false;
    };
    let Ok(function) = function.dyn_into::<js_sys::Function>() else {
        return false;
    };
    function
        .call1(&window, &"(pointer: fine)".into())
        .ok()
        .and_then(|result| Reflect::get(&result, &"matches".into()).ok())
        .and_then(|matches| matches.as_bool())
        .unwrap_or(false)
}

/// `MapBrowserPage.openPage`.
pub(super) fn open(page: &Shared) {
    {
        let mut guard = page.borrow_mut();
        let page = &mut *guard;
        if page.map_browser.open || page.catalog.is_undefined() || page.state.is_undefined() {
            return;
        }
        let view = View::new(page);
        let catalog = page.catalog.clone();
        let kind = match text(&view.attributes, "mapType") {
            kind if kind.is_empty() => "skirmish".to_string(),
            kind => kind,
        };
        let filter = text(&get(&view.state, "gui"), "map_filter");
        let current = text(&view.attributes, "map");
        let browser = &mut page.map_browser;
        browser.allow_selection = view.host;
        browser.open = true;
        show(&browser.root, true);
        browser.render_mode();
        let kinds: Vec<(String, String)> = strings(&catalog, "map_types")
            .iter()
            .map(|name| option(name, tc("map", &title_case(name))))
            .collect();
        fill(&browser.document, &browser.map_type, &kinds, &kind);
        browser.render_filter_options(&catalog);
        browser.map_filter.set_value(&filter);
        if browser.map_filter.value() != filter {
            browser.render_filter_options(&catalog);
        }
        browser.search.set_value("");
        browser.resize_grid();
        browser.update_map_list(&catalog);
        let found = browser.list.iter().position(|entry| entry.file == current);
        browser.set_selected(found);
        browser.scroll_to_selected();
        if fine_pointer() {
            let _ = browser.search.focus();
        }
    }
}

/// `MapBrowser.closePage`: the selection is discarded.
pub(super) fn close(page: &mut Page) {
    let browser = &mut page.map_browser;
    browser.open = false;
    browser.search.set_value("");
    show(&browser.root, false);
}

/// The host stepping down or passing the powers while the dialog is open.
pub(super) fn sync_host(page: &mut Page, view: &View) {
    let browser = &mut page.map_browser;
    if browser.open && browser.allow_selection != view.host {
        browser.allow_selection = view.host;
        browser.render_mode();
    }
}

/// `MapBrowser.submitMapSelection` and `MapBrowserPage.onSubmitMapSelection`.
fn submit(page: &Shared) {
    let entry = {
        let guard = page.borrow();
        let browser = &guard.map_browser;
        if !browser.open || !browser.allow_selection || !View::new(&guard).host {
            return;
        }
        let Some(entry) = browser.selected.map(|index| browser.list[index].clone()) else {
            return;
        };
        entry
    };
    send(
        page,
        &object(&[
            ("type", "change".into()),
            ("name", "browse_map".into()),
            ("value", entry.file.into()),
            ("map_type", entry.map_type.into()),
            ("map_filter", entry.filter.into()),
        ]),
    );
    close(&mut page.borrow_mut());
}

/// The listeners, made once.
pub(super) fn wire(page: &Shared) {
    let (
        root,
        grid,
        search,
        map_type,
        map_filter,
        zoom_in,
        zoom_out,
        pick_random,
        cancel,
        select,
        preview,
    ) = {
        let guard = page.borrow();
        let browser = &guard.map_browser;
        (
            browser.root.clone(),
            browser.grid.clone(),
            browser.search.clone(),
            browser.map_type.clone(),
            browser.map_filter.clone(),
            browser.zoom_in.clone(),
            browser.zoom_out.clone(),
            browser.pick_random.clone(),
            browser.cancel.clone(),
            browser.select.clone(),
            guard.dom.preview.clone(),
        )
    };
    {
        let mut guard = page.borrow_mut();
        let browser = &mut guard.map_browser;
        browser.zoom_in.set_text_content(Some(&tc("zoom in", "+")));
        let _ = browser
            .zoom_in
            .set_attribute("title", &translate("Increase map preview size."));
        browser
            .zoom_out
            .set_text_content(Some(&tc("zoom out", "\u{2212}")));
        let _ = browser
            .zoom_out
            .set_attribute("title", &translate("Decrease map preview size."));
        browser
            .select
            .set_text_content(Some(&tc("map selection dialog", "Select")));
        browser.render_mode();
    }

    let shared = Rc::clone(page);
    listen(&preview, "click", move |_| open(&shared));

    let shared = Rc::clone(page);
    listen(&map_type, "change", move |_| {
        let mut guard = shared.borrow_mut();
        let catalog = guard.catalog.clone();
        let browser = &mut guard.map_browser;
        browser.render_filter_options(&catalog);
        browser.on_change(&catalog);
    });
    let shared = Rc::clone(page);
    listen(&map_filter, "change", move |_| {
        let mut guard = shared.borrow_mut();
        let catalog = guard.catalog.clone();
        guard.map_browser.on_change(&catalog);
    });
    let shared = Rc::clone(page);
    listen(&search, "input", move |_| {
        let mut guard = shared.borrow_mut();
        let catalog = guard.catalog.clone();
        guard.map_browser.on_change(&catalog);
    });
    let shared = Rc::clone(page);
    listen(&zoom_in, "click", move |_| {
        shared.borrow_mut().map_browser.zoom(-1)
    });
    let shared = Rc::clone(page);
    listen(&zoom_out, "click", move |_| {
        shared.borrow_mut().map_browser.zoom(1)
    });
    let shared = Rc::clone(page);
    listen(&pick_random, "click", move |_| {
        let mut guard = shared.borrow_mut();
        let browser = &mut guard.map_browser;
        let len = browser.list.len();
        if len == 0 {
            return;
        }
        let index = ((Math::random() * len as f64).floor() as usize).min(len - 1);
        browser.set_selected(Some(index));
        browser.scroll_to_selected();
    });
    let shared = Rc::clone(page);
    listen(&cancel, "click", move |_| close(&mut shared.borrow_mut()));
    let shared = Rc::clone(page);
    listen(&select, "click", move |_| submit(&shared));
    let shared = Rc::clone(page);
    listen(&grid, "click", move |event| {
        if let Some(index) = MapBrowser::tile_index(&event) {
            shared.borrow_mut().map_browser.set_selected(Some(index));
        }
    });
    let shared = Rc::clone(page);
    listen(&grid, "dblclick", move |event| {
        if let Some(index) = MapBrowser::tile_index(&event) {
            shared.borrow_mut().map_browser.set_selected(Some(index));
            submit(&shared);
        }
    });
    let shared = Rc::clone(page);
    let backdrop = root.clone();
    listen(&root, "click", move |event| {
        // Only a click on the fade itself, not inside the dialog.
        let outside = event
            .target()
            .and_then(|clicked| clicked.dyn_into::<Element>().ok())
            .is_some_and(|clicked| clicked == *backdrop.unchecked_ref::<Element>());
        if outside {
            close(&mut shared.borrow_mut());
        }
    });

    let shared = Rc::clone(page);
    let on_key = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        let escape = Reflect::get(&event, &"key".into())
            .ok()
            .and_then(|key| key.as_string())
            .is_some_and(|key| key == CANCEL_KEY);
        if escape && shared.borrow().map_browser.open {
            event.prevent_default();
            close(&mut shared.borrow_mut());
        }
    });
    let document = page.borrow().map_browser.document.clone();
    let _ = document.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    on_key.forget();

    let shared = Rc::clone(page);
    let on_resize = Closure::<dyn FnMut(Event)>::new(move |_| {
        let mut guard = shared.borrow_mut();
        if guard.map_browser.open {
            guard.map_browser.resize_grid();
        }
    });
    if let Some(window) = web_sys::window() {
        let _ =
            window.add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref());
    }
    on_resize.forget();
}
