//! Explicit SSH launch UI. Opening the dialog never starts a connection.

use gtk::prelude::*;
use gtk4 as gtk;

use crate::ssh_hosts::{self, SshTarget};

pub fn show(parent: &libadwaita::ApplicationWindow, connect: impl Fn(SshTarget) + 'static) {
    let dialog = gtk::Window::builder()
        .title("Connect via SSH")
        .transient_for(parent)
        .modal(true)
        .default_width(440)
        .resizable(false)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(20);
    content.set_margin_end(20);

    let destination = gtk::Entry::builder()
        .placeholder_text("SSH alias or user@hostname")
        .activates_default(true)
        .build();
    let label = gtk::Label::new(Some("Host"));
    label.set_halign(gtk::Align::Start);
    label.set_mnemonic_widget(Some(&destination));
    content.append(&label);
    content.append(&destination);

    let status = gtk::Label::builder()
        .wrap(true)
        .halign(gtk::Align::Start)
        .build();
    match ssh_hosts::load_hosts() {
        Ok(hosts) if !hosts.is_empty() => {
            let choices: Vec<&str> = std::iter::once("Choose from ~/.ssh/config…")
                .chain(hosts.iter().map(String::as_str))
                .collect();
            let picker = gtk::DropDown::from_strings(&choices);
            let entry = destination.clone();
            picker.connect_selected_notify(move |picker| {
                if let Some(index) = picker.selected().checked_sub(1) {
                    if let Some(host) = hosts.get(index as usize) {
                        entry.set_text(host);
                    }
                }
            });
            content.append(&picker);
        }
        Err(err) => status.set_text(&format!(
            "Could not read SSH config: {err}. Enter a host manually."
        )),
        _ => {}
    }
    let port = gtk::Entry::builder()
        .placeholder_text("Port (optional; uses SSH config by default)")
        .activates_default(true)
        .build();
    content.append(&port);
    let hint = gtk::Label::builder()
        .label("Uses your OpenSSH configuration. Aliases from included files can be entered above. Authentication happens in the new terminal.")
        .wrap(true)
        .halign(gtk::Align::Start)
        .build();
    hint.add_css_class("dim-label");
    content.append(&hint);
    content.append(&status);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let launch = gtk::Button::with_label("Connect");
    launch.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&launch);
    content.append(&buttons);
    dialog.set_child(Some(&content));
    dialog.set_default_widget(Some(&launch));
    let weak_dialog = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(dialog) = weak_dialog.upgrade() {
            dialog.close();
        }
    });
    let weak_dialog = dialog.downgrade();
    launch.connect_clicked(
        move |_| match SshTarget::parse(&destination.text(), &port.text()) {
            Ok(target) => {
                if let Some(dialog) = weak_dialog.upgrade() {
                    dialog.close();
                }
                connect(target);
            }
            Err(err) => status.set_text(&err),
        },
    );
    dialog.present();
}
