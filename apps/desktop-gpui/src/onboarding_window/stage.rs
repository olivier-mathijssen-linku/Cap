//! The permissions step: one permission at a time on the right, the checklist
//! on the left, in the paper-and-ink look of Cap's Chrome extension pages.
//!
//! Only presentation and focus live here. Requests, polling and the
//! grant/denied classification are still `OnboardingWindow::act`, the poll
//! task and [`PermissionsState`](crate::permissions_ui::PermissionsState).

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, rgb,
    rgba,
};

use super::OnboardingWindow;
use crate::doodle::{self, Doodle, Palette};
use crate::onboarding_doodles as art;
use crate::permissions::{self, OSPermission, OSPermissionStatus};
use crate::permissions_ui::RowAction;
use crate::theme::Theme;

/// How long a freshly granted permission stays on stage, check drawn, before
/// the next one slides in.
const ADVANCE_AFTER: Duration = Duration::from_millis(1400);
const LIST_W: f32 = 296.;
const STAGE_W: f32 = 420.;

#[derive(Clone, Copy)]
pub struct Paper {
    pub bg: Hsla,
    pub card: Hsla,
    pub ink: Hsla,
    pub soft: Hsla,
    pub accent: Hsla,
    pub red: Hsla,
    pub green: Hsla,
    pub track: Hsla,
}

impl Paper {
    pub fn new(theme: &Theme) -> Self {
        if theme.is_dark() {
            Self {
                bg: rgb(0x18181b).into(),
                card: rgba(0xffffff0d).into(),
                ink: rgb(0xf2f2f3).into(),
                soft: rgb(0x8e8e96).into(),
                accent: rgb(0x7193dc).into(),
                red: rgb(0xff6369).into(),
                green: rgb(0x3dd68c).into(),
                track: rgb(0x3a3a40).into(),
            }
        } else {
            Self {
                bg: rgb(0xfbfaf7).into(),
                card: rgba(0xffffffa6).into(),
                ink: rgb(0x20242c).into(),
                soft: rgb(0x8b8f98).into(),
                accent: rgb(0x3b66c4).into(),
                red: rgb(0xe5484d).into(),
                green: rgb(0x2f9e69).into(),
                track: rgb(0xdcdcd4).into(),
            }
        }
    }

    pub fn palette(&self) -> Palette {
        Palette {
            ink: self.ink,
            soft: self.soft,
            accent: self.accent,
            red: self.red,
            green: self.green,
            track: self.track,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Permission(OSPermission),
    Drive,
    AllSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Pending,
    Active,
    Granted,
    Skipped,
}

pub(super) struct StageState {
    /// Pinned by a row click, or by a grant landing on the permission on
    /// stage (held for [`ADVANCE_AFTER`]). `None` follows the first
    /// permission still to do.
    focused: Option<OSPermission>,
    skipped: Vec<OSPermission>,
    pub(super) waiting_on: Option<OSPermission>,
    shown: Option<(Stage, bool)>,
    shown_at: Instant,
    marks: [(Mark, Instant); 4],
    granted: [bool; 4],
    /// Screen Recording was already granted when Cap launched. A grant that
    /// lands mid-session only reaches capture after a restart.
    screen_at_launch: bool,
    advance: Option<gpui::Task<()>>,
    drive: DriveState,
}

/// The optional Google Drive step after the permissions. Leaving the step
/// keeps a sign-in running; dropping `task` aborts it.
struct DriveState {
    connected: bool,
    skipped: bool,
    focused: bool,
    /// Holds the connected card on stage for [`ADVANCE_AFTER`].
    holding: bool,
    url: Option<String>,
    error: Option<String>,
    mark: (Mark, Instant),
    task: Option<gpui::Task<()>>,
    hold: Option<gpui::Task<()>>,
}

impl StageState {
    pub(super) fn new(granted: [bool; 4]) -> Self {
        let now = Instant::now();
        Self {
            focused: None,
            skipped: Vec::new(),
            waiting_on: None,
            shown: None,
            shown_at: now,
            marks: [(Mark::Pending, now); 4],
            granted,
            screen_at_launch: granted[0],
            advance: None,
            drive: DriveState {
                connected: crate::direct_drive::enabled() && cap_direct_drive::is_connected(),
                skipped: false,
                focused: false,
                holding: false,
                url: None,
                error: None,
                mark: (Mark::Pending, now),
                task: None,
                hold: None,
            },
        }
    }
}

fn index(permission: OSPermission) -> usize {
    OSPermission::ALL
        .iter()
        .position(|&candidate| candidate == permission)
        .unwrap_or(0)
}

struct Copy {
    title: &'static str,
    body: &'static str,
    done_title: &'static str,
    done_body: &'static str,
    allow: &'static str,
    doodle: &'static Doodle,
}

fn label(permission: OSPermission) -> &'static str {
    match permission {
        OSPermission::ScreenRecording => "Schermopname",
        OSPermission::Accessibility => "Toegankelijkheid",
        OSPermission::Microphone => "Microfoon",
        OSPermission::Camera => "Camera",
    }
}

fn copy_for(permission: OSPermission) -> Copy {
    match permission {
        OSPermission::ScreenRecording => Copy {
            title: "Laat Kiekes je scherm zien",
            body: "Met schermopname kan Kiekes je schermen, vensters en het geluid van je Mac opnemen.",
            done_title: "Schermopname staat aan",
            done_body: "Kiekes kan je scherm opnemen.",
            allow: "Schermopname toestaan",
            doodle: &art::SCREEN,
        },
        OSPermission::Accessibility => Copy {
            title: "Laat Kiekes je cursor volgen",
            body: "Met toegankelijkheid ziet Kiekes je klikken en je cursor, alleen op deze Mac, zodat automatisch inzoomen weet waar het moet kijken.",
            done_title: "Toegankelijkheid staat aan",
            done_body: "Automatisch inzoomen kan je klikken volgen.",
            allow: "Toegankelijkheid toestaan",
            doodle: &art::CURSOR,
        },
        OSPermission::Microphone => Copy {
            title: "Laat Kiekes je horen",
            body: "Zet je stem in je opnames. Je kunt dit overslaan en later aanzetten.",
            done_title: "De microfoon staat aan",
            done_body: "Je stem kan in je opnames.",
            allow: "Microfoon toestaan",
            doodle: &art::MICROPHONE,
        },
        OSPermission::Camera => Copy {
            title: "Laat Kiekes je zien",
            body: "Zet jezelf in een hoekje van je opname. Niet nodig? Sla het gerust over.",
            done_title: "De camera staat aan",
            done_body: "Je kunt je camera aan elke opname toevoegen.",
            allow: "Camera toestaan",
            doodle: &art::CAMERA,
        },
    }
}

impl OnboardingWindow {
    pub(super) fn needs_relaunch(&self) -> bool {
        !self.stage.screen_at_launch
            && self.state.status(OSPermission::ScreenRecording).permitted()
            && self.state.status(OSPermission::ScreenRecording) != OSPermissionStatus::NotNeeded
    }

    fn is_skipped(&self, permission: OSPermission) -> bool {
        self.stage.skipped.contains(&permission)
    }

    fn drive_offered(&self) -> bool {
        crate::direct_drive::enabled()
    }

    pub(super) fn current_stage(&self) -> Stage {
        let shown: Vec<(OSPermission, OSPermissionStatus)> = self.state.shown().collect();
        if let Some(focused) = self.stage.focused
            && shown.iter().any(|(permission, _)| *permission == focused)
        {
            return Stage::Permission(focused);
        }
        let drive = &self.stage.drive;
        if self.drive_offered() && (drive.focused || drive.holding) {
            return Stage::Drive;
        }
        shown
            .iter()
            .find(|(permission, status)| !status.permitted() && !self.is_skipped(*permission))
            .map(|(permission, _)| Stage::Permission(*permission))
            .unwrap_or(
                if self.drive_offered() && !drive.connected && !drive.skipped {
                    Stage::Drive
                } else {
                    Stage::AllSet
                },
            )
    }

    /// Reconcile focus, marks and entrance clocks with the latest grants.
    /// Called from `render`, so every path that folds in a sweep is covered.
    pub(super) fn sync_stage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let granted: [bool; 4] =
            std::array::from_fn(|i| self.state.status(OSPermission::ALL[i]).permitted());
        for (i, &permission) in OSPermission::ALL.iter().enumerate() {
            if !granted[i] || self.stage.granted[i] {
                continue;
            }
            if self.stage.waiting_on == Some(permission) {
                self.stage.waiting_on = None;
            }
            // The grant already moved `current_stage` on to the next missing
            // permission, so the card the user was looking at is the shown one.
            let on_screen = self.stage.shown.map(|(stage, _)| stage);
            if on_screen == Some(Stage::Permission(permission))
                || self.current_stage() == Stage::Permission(permission)
            {
                self.stage.focused = Some(permission);
                self.stage.advance = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(ADVANCE_AFTER).await;
                    this.update(cx, |this, cx| {
                        if this.stage.focused == Some(permission) {
                            this.stage.focused = None;
                            cx.notify();
                        }
                        this.stage.advance = None;
                    })
                    .ok();
                }));
            }
        }
        self.stage.granted = granted;

        let stage = self.current_stage();
        let stage_done = match stage {
            Stage::Permission(permission) => granted[index(permission)],
            Stage::Drive => self.stage.drive.connected,
            Stage::AllSet => true,
        };
        if self.stage.shown != Some((stage, stage_done)) {
            if self
                .stage
                .shown
                .is_none_or(|(previous, _)| previous != stage)
            {
                self.stage.shown_at = now;
            }
            self.stage.shown = Some((stage, stage_done));
        }
        if let Stage::Permission(on_stage) = stage
            && self
                .stage
                .waiting_on
                .is_some_and(|waiting| waiting != on_stage)
        {
            self.stage.waiting_on = None;
        }
        if let Some(waiting) = self.stage.waiting_on
            && !waiting.required()
            && self.pending.is_none()
            && self.state.status(waiting) == OSPermissionStatus::Denied
        {
            self.stage.waiting_on = None;
        }
        for (i, &permission) in OSPermission::ALL.iter().enumerate() {
            let mark = if granted[i] {
                Mark::Granted
            } else if self.is_skipped(permission) {
                Mark::Skipped
            } else if stage == Stage::Permission(permission) {
                Mark::Active
            } else {
                Mark::Pending
            };
            if self.stage.marks[i].0 != mark {
                self.stage.marks[i] = (mark, now);
            }
        }
        let drive = &mut self.stage.drive;
        let mark = if drive.connected {
            Mark::Granted
        } else if drive.skipped {
            Mark::Skipped
        } else if stage == Stage::Drive {
            Mark::Active
        } else {
            Mark::Pending
        };
        if drive.mark.0 != mark {
            drive.mark = (mark, now);
        }
    }

    fn focus_permission(&mut self, permission: OSPermission, cx: &mut Context<Self>) {
        self.stage.advance = None;
        self.stage.drive.focused = false;
        self.stage.drive.holding = false;
        self.stage.focused = Some(permission);
        self.stage.skipped.retain(|&skipped| skipped != permission);
        cx.notify();
    }

    fn skip_permission(&mut self, permission: OSPermission, cx: &mut Context<Self>) {
        if permission.required() {
            return;
        }
        self.stage.advance = None;
        if !self.is_skipped(permission) {
            self.stage.skipped.push(permission);
        }
        if self.stage.focused == Some(permission) {
            self.stage.focused = None;
        }
        if self.stage.waiting_on == Some(permission) {
            self.stage.waiting_on = None;
        }
        cx.notify();
    }

    fn focus_drive(&mut self, cx: &mut Context<Self>) {
        self.stage.advance = None;
        self.stage.focused = None;
        self.stage.drive.focused = true;
        self.stage.drive.skipped = false;
        cx.notify();
    }

    fn skip_drive(&mut self, cx: &mut Context<Self>) {
        let drive = &mut self.stage.drive;
        drive.task = None;
        drive.url = None;
        drive.error = None;
        drive.focused = false;
        drive.skipped = true;
        cx.notify();
    }

    fn connect_drive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stage.drive.task.is_some() {
            return;
        }
        self.stage.drive.error = None;
        self.stage.drive.url = None;
        self.stage.drive.task = Some(cx.spawn_in(window, async move |this, cx| {
            let authorization =
                match gpui_tokio::Tokio::spawn(cx, cap_direct_drive::begin_authorization()).await {
                    Ok(Ok(authorization)) => authorization,
                    Ok(Err(error)) => {
                        this.update(cx, |this, cx| this.drive_failed(error, cx))
                            .ok();
                        return;
                    }
                    Err(_) => return,
                };
            let url = authorization.url.clone();
            this.update(cx, |this, cx| {
                cx.open_url(&url);
                this.stage.drive.url = Some(url);
                cx.notify();
            })
            .ok();
            let completed = gpui_tokio::Tokio::spawn(cx, async move {
                authorization.complete(&AtomicBool::new(false)).await
            })
            .await;
            this.update_in(cx, |this, window, cx| match completed {
                Ok(Ok(())) => this.drive_connected(window, cx),
                Ok(Err(error)) => this.drive_failed(error, cx),
                Err(_) => {}
            })
            .ok();
        }));
        cx.notify();
    }

    fn drive_failed(&mut self, error: String, cx: &mut Context<Self>) {
        let drive = &mut self.stage.drive;
        drive.task = None;
        drive.url = None;
        drive.error = Some(error);
        cx.notify();
    }

    fn drive_connected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let drive = &mut self.stage.drive;
        drive.task = None;
        drive.url = None;
        drive.error = None;
        drive.connected = true;
        drive.skipped = false;
        drive.focused = false;
        drive.holding = true;
        drive.hold = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(ADVANCE_AFTER).await;
            this.update(cx, |this, cx| {
                this.stage.drive.holding = false;
                this.stage.drive.hold = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn render_permissions(&self, cx: &mut Context<Self>) -> AnyElement {
        let paper = Paper::new(&self.theme);
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .min_h_full()
            .px(px(40.))
            .py(px(24.))
            .gap(px(44.))
            .child(self.render_checklist(&paper, cx))
            .child(self.render_stage(&paper, cx))
            .into_any_element()
    }

    fn render_checklist(&self, paper: &Paper, cx: &mut Context<Self>) -> Div {
        let (title, lede) = if self.permissions_only {
            (
                "Kiekes mist een toestemming",
                "Er is iets uitgezet in Systeeminstellingen. Zet het weer aan om te kunnen opnemen.",
            )
        } else {
            (
                "Kiekes klaarzetten",
                "macOS vraagt eerst om een paar keer ‘oké’. Het kost maar een minuutje.",
            )
        };
        let stage = self.current_stage();
        div()
            .flex()
            .flex_col()
            .w(px(LIST_W))
            .flex_none()
            .child(
                div()
                    .text_size(px(28.))
                    .line_height(px(34.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(paper.ink)
                    .child(title),
            )
            .child(
                div()
                    .mt(px(10.))
                    .text_size(px(14.))
                    .line_height(px(21.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(paper.soft)
                    .child(lede),
            )
            .child(
                div()
                    .mt(px(26.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(self.state.shown().map(|(permission, status)| {
                        self.render_check_row(
                            paper,
                            permission,
                            status,
                            stage == Stage::Permission(permission),
                            cx,
                        )
                    }))
                    .when(self.drive_offered(), |list| {
                        list.child(self.render_drive_row(paper, stage == Stage::Drive, cx))
                    }),
            )
    }

    fn render_drive_row(&self, paper: &Paper, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let drive = &self.stage.drive;
        let caption = if drive.connected {
            ("Gekoppeld", paper.green)
        } else if drive.skipped {
            ("Overgeslagen", paper.soft)
        } else if drive.task.is_some() {
            ("Bezig in je browser", paper.soft)
        } else {
            ("Mag ook later", paper.soft)
        };
        check_row(
            paper,
            "perm-row-drive",
            "Google Drive",
            caption,
            drive.mark,
            !self.animated,
            active,
        )
        .when(!active, |row| {
            row.on_click(cx.listener(|this, _, _, cx| this.focus_drive(cx)))
        })
        .into_any_element()
    }

    fn render_check_row(
        &self,
        paper: &Paper,
        permission: OSPermission,
        status: OSPermissionStatus,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let caption = if status.permitted() {
            ("Geregeld", paper.green)
        } else if self.is_skipped(permission) {
            ("Overgeslagen", paper.soft)
        } else if status == OSPermissionStatus::Denied {
            ("Uit in Systeeminstellingen", paper.red)
        } else if permission.required() {
            ("Nodig om op te nemen", paper.soft)
        } else {
            ("Mag ook later", paper.soft)
        };
        check_row(
            paper,
            SharedString::from(format!("perm-row-{}", permission.label())),
            label(permission),
            caption,
            self.stage.marks[index(permission)],
            !self.animated,
            active,
        )
        .when(!active, |row| {
            row.on_click(cx.listener(move |this, _, _, cx| this.focus_permission(permission, cx)))
        })
        .into_any_element()
    }

    fn render_stage(&self, paper: &Paper, cx: &mut Context<Self>) -> AnyElement {
        let stage = self.current_stage();
        let shown_at = self.stage.shown_at;
        let still = !self.animated;
        let card = div()
            .id("perm-stage")
            .flex()
            .flex_col()
            .items_center()
            .w(px(STAGE_W))
            .flex_none()
            .min_h(px(420.))
            .justify_center()
            .px(px(36.))
            .py(px(30.))
            .rounded(px(24.))
            .border(px(1.5))
            .border_color(paper.track)
            .bg(paper.card);

        let card = match stage {
            Stage::AllSet if self.needs_relaunch() => card
                .child(doodle::doodle(
                    &art::ALL_SET,
                    paper.palette(),
                    shown_at,
                    still,
                    168.,
                    146.,
                ))
                .child(stage_title(paper, "Nog één keer opnieuw starten"))
                .child(stage_body(
                    paper,
                    "macOS zet schermopname aan zodra Kiekes opnieuw start. Je gaat daarna hier verder.",
                ))
                .child(div().mt(px(24.)).child(paper_button(
                    paper,
                    "perm-relaunch-done",
                    "Kiekes opnieuw starten",
                    true,
                    true,
                    cx.listener(|_, _, _, cx| permissions::relaunch(cx)),
                ))),
            Stage::AllSet => card
                .child(doodle::doodle(
                    &art::ALL_SET,
                    paper.palette(),
                    shown_at,
                    still,
                    168.,
                    146.,
                ))
                .child(stage_title(paper, "Alles staat klaar"))
                .child(stage_body(
                    paper,
                    if self.permissions_only {
                        "Dank je. Kiekes kan weer opnemen."
                    } else {
                        "Kiekes heeft alles wat het nodig heeft. Nu even kort wat je ermee kunt."
                    },
                )),
            Stage::Drive => self.render_drive_stage(card, paper, cx),
            Stage::Permission(permission) => {
                let copy = copy_for(permission);
                let status = self.state.status(permission);
                let granted = status.permitted();
                let card = card
                    .child(doodle::doodle(
                        copy.doodle,
                        paper.palette(),
                        shown_at,
                        still,
                        168.,
                        146.,
                    ))
                    .child(stage_title(
                        paper,
                        if granted {
                            copy.done_title
                        } else if self.stage.waiting_on == Some(permission) && permission.required()
                        {
                            "Zet Kiekes aan in Systeeminstellingen"
                        } else {
                            copy.title
                        },
                    ));
                if granted {
                    let done_body = if permission == OSPermission::ScreenRecording
                        && self.needs_relaunch()
                    {
                        "macOS past het toe als Kiekes opnieuw start. Dat kan zodra je hier klaar bent."
                    } else {
                        copy.done_body
                    };
                    card.child(stage_body(paper, done_body)).child(allowed_pill(
                        paper,
                        "Toegestaan",
                        self.stage.marks[index(permission)].1,
                        still,
                    ))
                } else {
                    self.render_stage_actions(card, paper, permission, status, &copy, cx)
                }
            }
        };

        card.into_any_element()
    }

    fn render_drive_stage(
        &self,
        card: gpui::Stateful<Div>,
        paper: &Paper,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let drive = &self.stage.drive;
        let still = !self.animated;
        let card = card.child(doodle::doodle(
            &art::UPLOAD,
            paper.palette(),
            self.stage.shown_at,
            still,
            168.,
            146.,
        ));
        if drive.connected {
            return card
                .child(stage_title(paper, "Google Drive is gekoppeld"))
                .child(stage_body(
                    paper,
                    "Je opnames gaan naar Google Drive. Bij het exporteren kies je zelf de map.",
                ))
                .child(allowed_pill(paper, "Gekoppeld", drive.mark.1, still));
        }
        let connecting = drive.task.is_some();
        let (title, body) = if connecting {
            (
                "Rond het af in je browser",
                "Log in met je Google-account van Linku en kies Toestaan. Kom daarna hier terug.",
            )
        } else {
            (
                "Koppel Google Drive",
                "Kiekes zet je opnames in Google Drive, zodat je ze makkelijk kunt delen. Je kunt dit ook later doen in Instellingen.",
            )
        };
        let reopen = drive.url.clone();
        let card = card
            .child(stage_title(paper, title))
            .child(stage_body(paper, body))
            .child(
                div()
                    .mt(px(24.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .child(if connecting {
                        paper_button(
                            paper,
                            "drive-reopen",
                            "Browser opnieuw openen",
                            true,
                            reopen.is_some(),
                            move |_, _, cx| {
                                if let Some(url) = &reopen {
                                    cx.open_url(url);
                                }
                            },
                        )
                    } else {
                        paper_button(
                            paper,
                            "drive-connect",
                            "Google Drive koppelen",
                            true,
                            true,
                            cx.listener(|this, _, window, cx| this.connect_drive(window, cx)),
                        )
                    })
                    .child(paper_button(
                        paper,
                        "drive-skip",
                        if connecting { "Annuleren" } else { "Later" },
                        false,
                        true,
                        cx.listener(|this, _, _, cx| this.skip_drive(cx)),
                    )),
            );
        let card = if connecting {
            card.child(
                div()
                    .mt(px(18.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .child(doodle::doodle(
                        &art::WAITING,
                        paper.palette(),
                        self.stage.shown_at,
                        still,
                        150.,
                        16.,
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(paper.soft)
                            .child("Wachten op Google"),
                    ),
            )
        } else {
            card
        };
        card.when_some(drive.error.clone(), |card, error| {
            card.child(
                div()
                    .mt(px(16.))
                    .max_w(px(320.))
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(paper.red)
                    .text_center()
                    .child(error),
            )
        })
    }

    fn render_stage_actions(
        &self,
        card: gpui::Stateful<Div>,
        paper: &Paper,
        permission: OSPermission,
        status: OSPermissionStatus,
        copy: &Copy,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let action = self.state.action(permission);
        let busy = self.pending.is_some();
        let waiting = self.stage.waiting_on == Some(permission);
        let guided = crate::permission_guide::eligible(permission);
        let waiting_required = waiting && permission.required();
        let body = if waiting_required && guided {
            "Sleep Kiekes uit het paneeltje onder Systeeminstellingen in de lijst. Staat hij er al? Zet dan de schakelaar aan."
        } else if waiting_required {
            "Zoek Kiekes in de lijst en zet hem aan. Vroeg macOS het eerst? Kies dan Open Systeeminstellingen."
        } else if status == OSPermissionStatus::Denied && !permission.required() {
            "Dit is eerder uitgezet. Zet Kiekes aan in Systeeminstellingen, of sla het voor nu over."
        } else if action == Some(RowAction::OpenSettings) && permission.required() {
            "Open Systeeminstellingen, zoek Kiekes in de lijst en zet hem aan."
        } else {
            copy.body
        };
        let primary_label = if waiting_required {
            "Systeeminstellingen opnieuw openen"
        } else {
            match action {
                Some(RowAction::OpenSettings) => "Systeeminstellingen openen",
                _ => copy.allow,
            }
        };
        let paper_copy = *paper;
        let card = card.child(stage_body(paper, body)).child(
            div()
                .mt(px(24.))
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(paper_button(
                    paper,
                    SharedString::from(format!("perm-action-{}", permission.label())),
                    primary_label,
                    true,
                    !busy,
                    cx.listener(move |this, _, window, cx| this.act(permission, window, cx)),
                ))
                .when(!permission.required(), |row| {
                    row.child(paper_button(
                        &paper_copy,
                        SharedString::from(format!("perm-skip-{}", permission.label())),
                        "Later",
                        false,
                        true,
                        cx.listener(move |this, _, _, cx| this.skip_permission(permission, cx)),
                    ))
                }),
        );

        // The AV request returned without macOS ever showing its prompt
        // (still undetermined once the request settled): say so and offer
        // the pane, instead of waiting on a dialog that is not there.
        let no_prompt = waiting
            && !permission.required()
            && !busy
            && !self.media_request_pending
            && status == OSPermissionStatus::NotDetermined;
        let card = if no_prompt {
            card.child(
                div()
                    .mt(px(18.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(paper.soft)
                            .child("Geen venster gezien? Zet Kiekes aan in Systeeminstellingen."),
                    )
                    .child(paper_button(
                        paper,
                        SharedString::from(format!("perm-settings-{}", permission.label())),
                        "Systeeminstellingen openen",
                        false,
                        true,
                        cx.listener(move |this, _, _, cx| {
                            this.state.note_settings_opened(permission);
                            permissions::open_permission_settings(permission);
                            cx.notify();
                        }),
                    )),
            )
        } else if waiting {
            let hint = if permission.required() || action == Some(RowAction::OpenSettings) {
                "Wachten op Systeeminstellingen"
            } else {
                "Kies Sta toe in het venster van macOS"
            };
            card.child(
                div()
                    .mt(px(18.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .child(doodle::doodle(
                        &art::WAITING,
                        paper.palette(),
                        self.stage.shown_at,
                        !self.animated,
                        150.,
                        16.,
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(paper.soft)
                            .child(hint),
                    ),
            )
        } else {
            card
        };

        if permission == OSPermission::ScreenRecording && waiting && self.state.relaunch_hint() {
            card.child(
                div()
                    .mt(px(18.))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(14.))
                    .border(px(1.5))
                    .border_color(paper.track)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(paper.soft)
                            .child("Staat hij aan maar gebeurt er niets? Start Kiekes opnieuw, dan pakt macOS het op."),
                    )
                    .child(paper_button(
                        paper,
                        "perm-relaunch",
                        "Opnieuw starten",
                        false,
                        true,
                        cx.listener(|_, _, _, cx| permissions::relaunch(cx)),
                    )),
            )
        } else {
            card
        }
    }
}

fn stage_title(paper: &Paper, title: &'static str) -> Div {
    div()
        .mt(px(22.))
        .text_size(px(22.))
        .line_height(px(28.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(paper.ink)
        .text_center()
        .child(title)
}

fn stage_body(paper: &Paper, body: &'static str) -> Div {
    div()
        .mt(px(8.))
        .max_w(px(320.))
        .text_size(px(14.))
        .line_height(px(21.))
        .font_weight(FontWeight::NORMAL)
        .text_color(paper.soft)
        .text_center()
        .child(body)
}

fn check_row(
    paper: &Paper,
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    (caption, caption_color): (&'static str, Hsla),
    (mark, mark_at): (Mark, Instant),
    still: bool,
    active: bool,
) -> gpui::Stateful<Div> {
    let doodle = match mark {
        Mark::Pending => &art::MARK_PENDING,
        Mark::Active => &art::MARK_ACTIVE,
        Mark::Granted => &art::MARK_GRANTED,
        Mark::Skipped => &art::MARK_SKIPPED,
    };
    let paper = *paper;
    div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.))
        .h(px(52.))
        .px(px(12.))
        .rounded(px(14.))
        .border(px(1.5))
        .border_color(if active {
            paper.accent
        } else {
            gpui::transparent_black()
        })
        .when(active, |row| row.bg(paper.card))
        .when(!active, |row| {
            row.cursor_pointer().hover(move |row| row.bg(paper.card))
        })
        .child(doodle::doodle(
            doodle,
            paper.palette(),
            mark_at,
            still,
            30.,
            30.,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(14.))
                        .line_height(px(18.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(paper.ink)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(caption_color)
                        .child(caption),
                ),
        )
}

fn allowed_pill(paper: &Paper, label: &'static str, shown_at: Instant, still: bool) -> Div {
    div()
        .mt(px(24.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .h(px(42.))
        .px(px(18.))
        .rounded_full()
        .border(px(1.5))
        .border_color(paper.green)
        .text_size(px(14.))
        .text_color(paper.green)
        .child(doodle::doodle(
            &art::MARK_GRANTED,
            paper.palette(),
            shown_at,
            still,
            22.,
            22.,
        ))
        .child(label)
}

/// The extension's `.cta`: an ink pill. Primary is filled; secondary is the
/// outlined `ghost`.
pub(super) fn paper_button(
    paper: &Paper,
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    primary: bool,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    let paper = *paper;
    let (bg, fg, border) = if primary {
        (paper.accent, gpui::white(), paper.accent)
    } else {
        (gpui::transparent_black(), paper.ink, paper.track)
    };
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .h(px(42.))
        .px(px(22.))
        .rounded_full()
        .border(px(1.5))
        .border_color(border)
        .bg(bg)
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label)
        .when(!enabled, |button| button.opacity(0.45))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(move |button| {
                    if primary {
                        button.opacity(0.88)
                    } else {
                        button.border_color(paper.soft)
                    }
                })
                .on_click(on_click)
        })
}
