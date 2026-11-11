//! The Body page: log weight / body fat, see the trend chart and history.

use chrono::{Local, NaiveDateTime, TimeZone};
use dioxus::prelude::*;
use hud_store::Store;
use hud_ui::{Page, use_store};

use crate::{Measurement, chart, history, remove, save};

pub fn page() -> Page {
    Page {
        id: "body",
        label: "Body",
        component: body_page,
    }
}

// ponytail: fixed until the settings screen can edit it (smaller = smoother trend).
const TREND_ALPHA: f64 = 0.1;
const INPUT_FORMAT: &str = "%Y-%m-%dT%H:%M"; // what <input type="datetime-local"> produces

fn body_page() -> Element {
    let store = use_store();
    let mut version = use_signal(|| 0u32); // bump to reload history after a write
    let mut when = use_signal(|| Local::now().format(INPUT_FORMAT).to_string());
    let mut weight = use_signal(String::new);
    let mut fat = use_signal(String::new);
    let mut note = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    let rows = use_memo({
        let store = store.clone();
        move || {
            version();
            history(&store.lock()).unwrap_or_default()
        }
    });

    let on_submit = {
        let store = store.clone();
        move |e: FormEvent| {
            e.prevent_default();
            match try_save(&mut store.lock(), &when(), &weight(), &fat(), &note()) {
                Ok(()) => {
                    weight.set(String::new());
                    fat.set(String::new());
                    note.set(String::new());
                    error.set(None);
                    version += 1;
                }
                Err(msg) => error.set(Some(msg)),
            }
        }
    };

    let readings = rows();
    let plot = chart(&readings, TREND_ALPHA, 600.0, 180.0);

    rsx! {
        h2 { "Body" }
        div { class: "card",
            form { onsubmit: on_submit,
                label {
                    "When"
                    input { r#type: "datetime-local", value: "{when}", oninput: move |e| when.set(e.value()) }
                }
                label {
                    "Weight (kg)"
                    input {
                        r#type: "text",
                        inputmode: "decimal",
                        placeholder: "80.5",
                        value: "{weight}",
                        oninput: move |e| weight.set(e.value()),
                    }
                }
                label {
                    "Body fat (%)"
                    input {
                        r#type: "text",
                        inputmode: "decimal",
                        placeholder: "optional",
                        value: "{fat}",
                        oninput: move |e| fat.set(e.value()),
                    }
                }
                label {
                    "Note"
                    input { r#type: "text", value: "{note}", oninput: move |e| note.set(e.value()) }
                }
                button { class: "primary", r#type: "submit", "Log" }
            }
            if let Some(msg) = error() {
                p { class: "error", "{msg}" }
            }
        }
        div { class: "card",
            if let Some(c) = plot {
                svg { class: "chart", view_box: "-10 -14 620 214",
                    polyline {
                        class: "trend",
                        points: c.trend.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect::<Vec<_>>().join(" "),
                    }
                    for (x , y) in c.raw.iter() {
                        circle { class: "raw", cx: "{x:.1}", cy: "{y:.1}", r: "3" }
                    }
                    text { class: "axis", x: "0", y: "-3", "{c.max_kg:.1} kg" }
                    text { class: "axis", x: "0", y: "196", "{c.min_kg:.1} kg" }
                }
                p { class: "muted", "Dots are readings; the line is the smoothed trend." }
            } else {
                p { class: "muted", "Log a weight to see your trend." }
            }
        }
        div { class: "card",
            table {
                thead {
                    tr {
                        th { "When" }
                        th { "Weight" }
                        th { "Body fat" }
                        th { "Note" }
                        th {}
                    }
                }
                tbody {
                    for m in readings.iter().rev() {
                        tr { key: "{m.id}",
                            td { "{fmt_time(m.measured_at)}" }
                            td {
                                if let Some(w) = m.weight_kg {
                                    "{w:.1} kg"
                                }
                            }
                            td {
                                if let Some(b) = m.body_fat_pct {
                                    "{b:.1} %"
                                }
                            }
                            td { "{m.note.clone().unwrap_or_default()}" }
                            td {
                                button {
                                    class: "link",
                                    onclick: {
                                        let store = store.clone();
                                        let id = m.id;
                                        move |_| {
                                            // ponytail: a failed delete is silent; surface it once there is a toast area.
                                            let _ = remove(&mut store.lock(), id);
                                            version += 1;
                                        }
                                    },
                                    "Delete"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn fmt_time(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// Parse the form fields and save a measurement. Errors are user-facing messages.
fn try_save(
    store: &mut Store,
    when: &str,
    weight: &str,
    fat: &str,
    note: &str,
) -> Result<(), String> {
    let at = NaiveDateTime::parse_from_str(when, INPUT_FORMAT)
        .map_err(|_| "Enter a valid date and time.".to_owned())?
        .and_local_timezone(Local)
        .earliest()
        .ok_or("That local time does not exist (daylight-saving gap).")?
        .timestamp();
    let number = |text: &str, what: &str| -> Result<Option<f64>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        text.replace(',', ".")
            .parse()
            .map(Some)
            .map_err(|_| format!("{what} must be a number."))
    };
    let note = note.trim();
    let m = Measurement::new(
        at,
        number(weight, "Weight")?,
        number(fat, "Body fat")?,
        (!note.is_empty()).then(|| note.to_owned()),
    );
    save(store, &m).map_err(|e| {
        let msg = e.to_string();
        let mut chars = msg.chars();
        chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str() + ".")
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use hud_core::Registry;
    use hud_ui::{App, AppState};

    use super::*;
    use crate::Body;

    fn store() -> Store {
        Store::open_in_memory(Registry::new(vec![Box::new(Body)]).unwrap()).unwrap()
    }

    #[test]
    fn page_renders_nav_chart_and_history() {
        let mut s = store();
        save(
            &mut s,
            &Measurement::new(1_700_000_000, Some(81.2), None, Some("after run".into())),
        )
        .unwrap();
        save(
            &mut s,
            &Measurement::new(1_700_086_400, Some(80.6), Some(17.5), None),
        )
        .unwrap();

        let mut vdom = VirtualDom::new(App).with_root_context(AppState::new(s, vec![page()]));
        vdom.rebuild_in_place();
        let html = dioxus_ssr::render(&vdom);
        for want in [
            "Body",
            "81.2 kg",
            "80.6 kg",
            "17.5 %",
            "after run",
            "<polyline",
        ] {
            assert!(html.contains(want), "missing `{want}` in: {html}");
        }
    }

    #[test]
    fn empty_page_invites_a_first_reading() {
        let mut vdom = VirtualDom::new(App).with_root_context(AppState::new(store(), vec![page()]));
        vdom.rebuild_in_place();
        assert!(dioxus_ssr::render(&vdom).contains("Log a weight"));
    }

    #[test]
    fn form_parsing_gives_readable_errors_and_accepts_decimal_commas() {
        let mut s = store();
        assert_eq!(
            try_save(&mut s, "nope", "80", "", ""),
            Err("Enter a valid date and time.".into())
        );
        let when = "2026-11-11T07:30";
        assert_eq!(
            try_save(&mut s, when, "abc", "", ""),
            Err("Weight must be a number.".into())
        );
        assert_eq!(
            try_save(&mut s, when, "", "", ""),
            Err("Enter a weight or a body-fat percentage.".into())
        );
        assert_eq!(try_save(&mut s, when, "80,5", "", " note "), Ok(()));
        let saved = history(&s).unwrap();
        assert_eq!(
            (saved[0].weight_kg, saved[0].note.as_deref()),
            (Some(80.5), Some("note"))
        );
    }
}
