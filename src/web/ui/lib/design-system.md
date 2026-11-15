# web/ui/lib — the design system

One place each thing is written down, so a page states intent and the theme states everything else.
The layers below are the whole vocabulary; a rule that is not here is a rule a page invented.

## The look: Immich's own

These pages open beside Immich's, so they are built the way Immich builds its own, from its
`@immich/ui` theme and its web app rather than invented:

| Thing           | Immich's value                                                                                  | Here                                            |
| --------------- | ----------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| Primary         | `#4250af` light, `#accbfa` dark; filled text white / black                                      | `--isa-accent`, `--isa-accent-ink`              |
| Page            | white / pure black; text `--immich-ui-dark` (`#3d3d3d` / `#dbdbdb`), not black                  | `--isa-bg`, `--isa-ink`                         |
| Card            | `bg-light-50` (`#fafafa`) / `#171717`, 1px border (`#d4d4d4` / `#262626`), 16px corners         | `--isa-surface`, `--isa-border`, `.isa-card`    |
| Settings group  | `SettingAccordion`: 2px primary/20 border (primary/60 and a shadow when open), icon, medium primary title, subtitle, turning chevron | `Section`, `.isa-accordion`        |
| Setting row     | `SettingSwitch`: label in primary, medium weight; description under it; switch at the end       | `Setting`, `.isa-setting`                       |
| Page chrome     | a 64px bar with a bottom border: back arrow, breadcrumb, ghost icon actions on the right        | `Shell`, `.isa-bar`                             |
| Button          | 12px corners, 14px medium; pill (`--round`) on auth screens; filled hovers to 80%, outline is a 10% tint on a primary border | `.isa-btn` |
| Input           | `immich-form-input`: a filled well held by a 1px ring, primary on focus, 16px text, 8px corners | `.isa-field`                                    |
| Toast, dialog   | a bordered card tinted per kind with a rounded icon tile; the dialog is header / body / footer split by hairlines on a 30% scrim | `.isa-notice`, `.isa-dialog` |
| Icons           | Material Design Icons (Apache-2.0), the set Immich draws with                                   | `icons.tsx`                                     |
| Font            | `GoogleSans`, falling back to system sans — the font file is Immich's and is not shipped here   | `--isa-font`                                    |

Two deliberate departures, both for accessibility: Immich's danger red (`danger-500`) is 3.9:1 on
white, so text and fills use its next step (`danger-600`, `#c81c15`); and every control keeps a 44px
floor where Immich's medium button is shorter. `check-contrast.mjs` measures every pair in both
schemes.

## The layers

| File                  | Owns                                                                                                                                                                       |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tokens.css`          | Every colour, radius, space step and duration, in both schemes. The only file under `src/web/ui` allowed a colour literal (`scripts/check-tokens.mjs`).                    |
| `ui.css`              | The shared components: card, section, row, button, field, setting, switch, notice, dialog, spinner, keyframes. A page styles itself from here plus its own stylesheet.           |
| `base.css`            | The document shell our own pages render into — `body`, `main`, the heading reset, `.isa-page-*`, the focus ring. Imports `ui.css`.                                           |
| `<page>.css`          | Only what is that page's. Every page imports `base.css`; `share.css` imports `ui.css` directly, because its document frames Immich's page and must not restyle it.               |

`ui.css` is inlined **before** `base.css` — it is what `base.css` imports — so a rule in `ui.css`
that a `base.css` rule of the same specificity also names always loses to it. Narrow-screen overrides
for `.isa-page-*` therefore live in `base.css`, and the `@media` block at the end of `ui.css` carries
only selectors `base.css` does not declare.

`--isa-accent` is load-bearing outside this file: `demo/e2e/browser-test.mjs` asserts a signed-out
page's CTA computes to `rgb(66, 80, 175)`. Changing the light accent changes that contract.

## Components

| Export          | Renders                                                    | Used by                                              |
| --------------- | ---------------------------------------------------------- | ---------------------------------------------------- |
| `Button`        | `<button class="isa-btn isa-btn--{fill}">`, `type="button"`; a passed `class` is added to, never replaces | every panel page, the accept page, `confirm.tsx` |
| `Shell`         | `.isa-bar` (back arrow to Immich, breadcrumb, household, ghost icon actions) and `<main class="isa-main">` | panel, me, root |
| `Section`       | `.isa-accordion`: an icon, a primary title and subtitle over a body that collapses; open by default | panel, me                              |
| `Icon`          | an inline Material Design Icon by name (`icons.tsx`)         | every page                                            |
| `LinkIcon`      | the addon's one glyph, `currentColor`                       | the accept and sign-in logos, the share banner        |
| `Card`          | `<section class="isa-card isa-enter">` with an optional lede   | panel, me, root                                      |
| `Setting`       | `<label class="isa-setting">` — text, description, control | panel `Settings.tsx`, me `App.tsx`                   |
| `Switch`        | `<input type="checkbox" role="switch" class="isa-switch">` | panel `Settings.tsx`, me `App.tsx`                   |
| `AlbumLink`     | `<a class="isa-album-link">` to `/albums/<albumId>`     | panel `SharedAlbums.tsx`, me `App.tsx`                 |
| `Notice`        | `#notice`, the toast, `role=status`/`alert`               | me `App.tsx`, panel `App.tsx` and `LinkServer.tsx`     |
| `useAnnouncer`  | the notice state, `announce()`, `dismiss()` and the success fade | me and panel, in place of two copies     |
| `confirm.tsx`   | `Confirm` — the one dialog, `.isa-scrim` + `.isa-dialog`   | me, panel                                            |

`Button`'s `fill` is the only knob: `filled`, `outlined`, `text`, `danger`, `dangerFilled`.
Destructive in a list is `danger` (tonal) so a page of rows is not a wall of red; the filled
`dangerFilled` is for a confirmation, where the action is the only thing on screen. Hover is a colour
change on the fill (`color-mix` of the token at 80%, or 20% for the tints) — no lift, no state layer,
no press animation.

`Button` does **not** forward a `ref`. Preact hands a function component's ref its own wrapper, not
the DOM node, and `confirm.tsx` — the one caller that needs the node, to take the caret — writes its
confirm button out as a host `<button>` instead. Anything else that needs the node does the same.

## The switch

- A **real `<input type="checkbox">`**, drawn by `ui.css` with `appearance: none`. No replacement
  element and no `div` with a click handler: the keyboard, the form and assistive technology get the
  control they already understand.
- `role="switch"` is what announces it as on/off rather than checked/unchecked.
- Track 52x32 with a 2px border, as Immich draws it. Off: `--isa-switch-track-off` fill, a visible
  `--isa-switch-border-off` outline, a 16px `--isa-switch-thumb-off` thumb. On: a tinted
  `--isa-switch-track-on` fill, no outline, a 24px primary thumb. The thumb's resting centres are 16px
  and 36px from the left edge in every state.
- The whole row is the target: `Setting` wraps the control in a `<label>`, so nobody has to hit 52
  pixels of switch.

## Space

- `--isa-space-1..8` is a 4px step (4, 8, 12, 16, 20, 24, 32, 40). Nothing else is a margin.
- A page is a full-width title bar over one column of `--isa-page-max` (720px) inside `--isa-gutter`,
  16px and 24px from 600px. The bar is `--isa-bar-height` (64px).
- Rows are `--isa-space-3` of padding with a hairline **between** them (`.isa-row + .isa-row`), never
  under the last; a row leads with a 40px `.isa-row-icon` tile.
- A plain card is `--isa-pad-card`, 24px and 16px under 420px; a section's accordion is 16px by 24px.
- Sections are `--isa-space-4` apart; a section's title is `.isa-section-title`, in primary, sentence case.

## Motion

- One curve, Tailwind's (`cubic-bezier(0.4, 0, 0.2, 1)`) — the one Immich's transitions use — held in
  `--isa-ease-standard` and `--isa-ease-emphasized`, and three durations: `--isa-duration-short`
  (100ms, the switch thumb), `-medium` (150ms, every colour change) and `-long` (250ms, an entrance).
- Four arrivals — the card (`isa-rise-in`), the sheet (`isa-sheet-in`), the dialog (`isa-dialog-in`,
  with its scrim's `isa-fade-in`) and the toast (`isa-notice-in`) — and one loop, the spinner
  (`isa-spin`). Everything else arrives and is then still. The toast also leaves (`isa-notice-out`),
  in the direction it was swiped.
- A control answers a press with a colour change only; nothing scales, lifts or ripples.
- `prefers-reduced-motion: reduce` collapses all of it, scoped to `.isa-page` — the class
  `Document.tsx` puts on `<body>` — so the rule cannot reach the framed Immich page under the banner.

## Album names

- `AlbumLink` takes a row title's place and points at `/albums/<albumId>` — Immich's own SPA
  route, so an album opens where the rest of the person's library already lives.
- **No `albumId` means plain text, not a broken link.** `/peers` and `/me/albums` read it from the
  mapping; a reunion row's is top-level on the row.
- **`canOpen` keeps the link and explains itself.** An admin panel lists albums owned by other people,
  and Immich refuses `/albums/<id>` for someone who is not a member — so a title this admin cannot
  open is still a title, still a link, and pressing it announces the reason rather than navigating.
  Demoting it to plain text would leave no sign the album was there at all.
- The id travels on the album (`OwnedAlbum.id`, `#[serde(skip_serializing)]`, so it never reaches a
  peer) rather than beside it: two albums on one server can share a name, and a map keyed on the name
  links to whichever came last.

## The toast

- `Notice` is how **every** action reports: a card that writes its outcome into itself puts the
  message where the row it describes no longer is, and unlinking deletes that row.
- It spans the gutter — `width`, not `max-width` — because a bar that hugs its words wraps a
  one-line outcome into six, and a phone is the only place it is read.
- `--isa-notice-surface` and `--isa-notice-ink` are the page's own surface and ink: Immich's toast is a
  bordered card, with a 32px tile (`--isa-ok` or `--isa-danger`) saying what kind of message it is.
- `useAnnouncer` numbers every announcement, and the `Notice` is keyed on that number: a second
  message REPLACES the first instead of inheriting whatever the first was doing on its way out —
  otherwise a bar mid-swipe takes the message that replaced it down with it.
- **Three ways out**, because each leaves someone out otherwise: the × , a horizontal swipe
  (pointer events, so the finger and the mouse are one path), and Escape. A short swipe springs
  back — a bar that vanishes under a slip is worse than no bar.
- `touch-action: pan-y` keeps a vertical drag the page's scroll.

## Narrow screens

- Under 560px the accept card and the share banner become **sheets**: square bottom corners, padded
  above `env(safe-area-inset-bottom)`.
- Under 600px the title bar keeps the back arrow, the breadcrumb and each action's icon; the household
  and the action labels hide, and the action keeps its `aria-label`.
- Under 420px a row's action drops to its own line, full width — a button sharing a line with a long
  album name leaves neither readable.
- `--isa-touch` (44px) is the floor for anything tappable, whatever its type size says.

## Test contracts

Class names and ids the browser lane drives, which must survive a redesign:

| Selector                                          | Where                                                        |
| ------------------------------------------------- | ------------------------------------------------------------ |
| `#immich-shared-albums-banner .card/.dismiss/input/button.join/.err` | `pages/share/Share.tsx`                      |
| `#who`, `#go`, `#out`, `#reunion`, `#joinseparate`, `#openapp` | `pages/accept/Accept.tsx`                    |
| `a.choice`                                        | `pages/root/App.tsx` — asserted for `display:flex` and the chevron's position |
| `label:has-text(...) input[type=checkbox]`        | `pages/panel/Settings.tsx` — the switches are still checkboxes |
| `#audit-visible`                                  | `pages/me/App.tsx` — the album-activity switch               |
| `a[href="/auth/login"]`                           | `pages/sign-in/` — asserted `rgb(66, 80, 175)`               |

Section order is a contract too: `sectionBetween` in `demo/e2e/browser-test.mjs` reads the page's
`innerText` from "Possible album reunions" to "Your shared albums", so a section stays open in the DOM
by default and keeps that order. It matches headings case-insensitively, because `innerText` returns what
was painted rather than what is in the markup.