"""Themed dual-country public-holiday selection dialog."""

from __future__ import annotations

import tkinter as tk
from tkinter import ttk
from typing import TYPE_CHECKING

import weeknumber_core as core
from weeknumber_core import SettingsStore
from weeknumber_dialog import TitledDialog
from weeknumber_widgets import PillButton, style_combobox

if TYPE_CHECKING:
    from collections.abc import Callable


class HolidayDialog(TitledDialog):
    """Single-instance, DPI-aware selector for primary and secondary calendars."""

    TITLE = "Public Holidays"
    WIDTH_PX = 520
    HEIGHT_PX = 300
    MARGIN_PX = 28

    def __init__(self, master: tk.Misc, settings: SettingsStore,
                 on_change: "Callable[[], None] | None" = None) -> None:
        work = self._init_window(master, settings)
        self._on_change = on_change
        countries = core.holiday_countries()
        self._code_to_name = dict(countries)
        self._name_to_code = {name: code for code, name in countries}
        self._country_names = [name for _code, name in countries]
        self._typeahead_text: dict[str, str] = {}
        self._typeahead_commands: dict[str, str] = {}

        self.primary_var = tk.StringVar(
            value=self._code_to_name.get(settings.holiday_country_primary, "Off"))
        self.secondary_var = tk.StringVar(
            value=self._code_to_name.get(settings.holiday_country_secondary, "None / Off"))
        self._build()

        width, height = self._px(self.WIDTH_PX), self._px(self.HEIGHT_PX)
        x = work.left + max(0, (work.right - work.left - width) // 2)
        y = work.top + max(0, (work.bottom - work.top - height) // 2)
        self.top.geometry(f"{width}x{height}+{x}+{y}")
        self._apply_theme()
        self.top.deiconify()
        self._sync_titlebar()
        self._raise()

    def _build(self) -> None:
        margin = self._px(self.MARGIN_PX)
        self.card = tk.Frame(self.top, padx=margin, pady=self._px(22))
        self.card.pack(fill="both", expand=True)

        self.title = tk.Label(self.card, text="Public Holidays",
                              font=self._font(19, bold=True), anchor="w")
        self.title.pack(fill="x")
        self.subtitle = tk.Label(
            self.card,
            text="Show nationwide non-working holidays from up to two countries.",
            font=self._font(11), anchor="w")
        self.subtitle.pack(fill="x", pady=(self._px(4), self._px(20)))

        self.form = tk.Frame(self.card)
        self.form.pack(fill="x")
        self.form.grid_columnconfigure(1, weight=1)

        self.primary_swatch = tk.Canvas(
            self.form, width=self._px(14), height=self._px(14),
            highlightthickness=0, bd=0)
        self.primary_swatch.grid(row=0, column=0, padx=(0, self._px(9)))
        self.primary_label = tk.Label(
            self.form, text="Primary (purple)", font=self._font(11, bold=True),
            anchor="w")
        self.primary_label.grid(row=0, column=1, sticky="w")
        self.primary_combo = ttk.Combobox(
            self.form, textvariable=self.primary_var,
            values=["Off", *self._country_names], state="readonly",
            font=self._font(11), height=16)
        self.primary_combo.grid(
            row=1, column=0, columnspan=2, sticky="ew",
            pady=(self._px(5), self._px(15)))
        self.primary_combo.bind("<<ComboboxSelected>>", self._selection_changed)
        self.primary_combo.configure(
            postcommand=lambda: self._prepare_typeahead(self.primary_combo))

        self.secondary_swatch = tk.Canvas(
            self.form, width=self._px(14), height=self._px(14),
            highlightthickness=0, bd=0)
        self.secondary_swatch.grid(row=2, column=0, padx=(0, self._px(9)))
        self.secondary_label = tk.Label(
            self.form, text="Secondary (teal)", font=self._font(11, bold=True),
            anchor="w")
        self.secondary_label.grid(row=2, column=1, sticky="w")
        self.secondary_combo = ttk.Combobox(
            self.form, textvariable=self.secondary_var,
            values=["None / Off", *self._country_names], state="readonly",
            font=self._font(11), height=16)
        self.secondary_combo.grid(
            row=3, column=0, columnspan=2, sticky="ew", pady=(self._px(5), 0))
        self.secondary_combo.bind("<<ComboboxSelected>>", self._selection_changed)
        self.secondary_combo.configure(
            postcommand=lambda: self._prepare_typeahead(self.secondary_combo))

        self.footer = tk.Frame(self.card)
        self.footer.pack(side="bottom", fill="x", pady=(self._px(18), 0))
        # Hand-drawn: Aqua's native button ignores the accent colours.
        self.save_button = PillButton(
            self.footer, "Save", self._font(11, bold=True), self.scale, self.save)
        self.save_button.pack(side="right")

    def _prepare_typeahead(self, combo: ttk.Combobox) -> None:
        """Prepare the country list before Tk displays the combobox popdown."""
        combo_path = str(combo)
        self._typeahead_text[combo_path] = ""
        self._bind_typeahead_listbox(combo)

    def _bind_typeahead_listbox(self, combo: ttk.Combobox) -> None:
        """Style and bind the actual ttk combobox popdown listbox."""
        combo_path = str(combo)
        try:
            popdown_path = combo.tk.call(
                "ttk::combobox::PopdownWindow", combo_path)
            listbox_path = f"{popdown_path}.f.l"
            if not combo.tk.getboolean(
                    combo.tk.call("winfo", "exists", listbox_path)):
                return
            self._style_popdown_listbox(listbox_path)
            # ttk runs the postcommand before it maps/configures the popdown.
            # Reapply after that work so the Windows theme cannot overwrite it.
            self.top.after_idle(
                lambda path=listbox_path: self._style_popdown_listbox(path))
        except tk.TclError:
            return

        command = self._typeahead_commands.get(combo_path)
        if command is None:
            command = self.top.register(
                lambda widget, char, keysym:
                    self._typeahead_key(combo, widget, char, keysym))
            self._typeahead_commands[combo_path] = command

        script = (
            f'if {{"[{command} %W %A %K]" eq "break"}} {{break}}'
        )
        combo.tk.call("bind", listbox_path, "<KeyPress>", script)

    def _style_popdown_listbox(self, listbox_path: str) -> None:
        """Apply the same palette as the time-zone dropdown list."""
        try:
            self.top.tk.call(
                listbox_path, "configure",
                "-background", self.theme["MENU_BG"],
                "-foreground", self.theme["MENU_FG"],
                "-selectbackground", self.theme["MENU_ACTIVE_BG"],
                "-selectforeground", self.theme["MENU_ACTIVE_FG"],
            )
        except tk.TclError:
            pass

    def _typeahead_key(self, combo: ttk.Combobox, listbox_path: str,
                       char: str, keysym: str) -> str | None:
        """Move the open list to the first case-insensitive prefix match."""
        combo_path = str(combo)
        search = self._typeahead_text.get(combo_path, "")

        if keysym == "BackSpace":
            search = search[:-1]
        elif char and char.isprintable():
            if not search and char.isspace():
                return "break"
            search += char
        else:
            return None

        self._typeahead_text[combo_path] = search
        if not search:
            return "break"

        folded = search.casefold()
        values = tuple(combo.cget("values"))
        match = next(
            (index for index, value in enumerate(values)
             if value.casefold().startswith(folded)),
            None,
        )
        if match is None:
            return "break"

        try:
            combo.tk.call(listbox_path, "selection", "clear", 0, "end")
            combo.tk.call(listbox_path, "selection", "set", match)
            combo.tk.call(listbox_path, "activate", match)
            combo.tk.call(listbox_path, "see", match)
        except tk.TclError:
            pass
        return "break"

    def _selection_changed(self, event=None) -> None:
        primary = self._name_to_code.get(self.primary_var.get())
        secondary = self._name_to_code.get(self.secondary_var.get())
        if primary is not None and primary == secondary:
            self.secondary_var.set("None / Off")
            self.top.bell()

    def save(self) -> None:
        primary = self._name_to_code.get(self.primary_var.get())
        secondary = self._name_to_code.get(self.secondary_var.get())
        valid = set(self._code_to_name)
        if ((primary is not None and primary not in valid)
                or (secondary is not None and secondary not in valid)
                or (primary is not None and primary == secondary)):
            self.top.bell()
            return
        changed = (
            primary != self.settings.holiday_country_primary
            or secondary != self.settings.holiday_country_secondary
        )
        self.settings.holiday_country_primary = primary
        self.settings.holiday_country_secondary = secondary
        self.settings.save()
        if changed and self._on_change is not None:
            self._on_change()
        self.close()

    def _apply_theme(self) -> None:
        super()._apply_theme()
        bg = self.theme["CARD_BG"]
        fg = self.theme["TITLE_FG"]
        muted = self.theme["YEAR_FG"]
        self.top.configure(bg=bg)
        for widget in (self.card, self.form, self.footer):
            widget.configure(bg=bg)
        for widget in (self.title, self.primary_label, self.secondary_label):
            widget.configure(bg=bg, fg=fg)
        self.subtitle.configure(bg=bg, fg=muted)

        for canvas, color in (
            (self.primary_swatch, self.theme["HOLIDAY_BG"]),
            (self.secondary_swatch, self.theme["HOLIDAY_SECONDARY_BG"]),
        ):
            canvas.configure(bg=bg)
            canvas.delete("all")
            inset = self._px(1)
            canvas.create_oval(
                inset, inset, self._px(13), self._px(13),
                fill=color, outline=self.theme["CARD_BORDER"])

        style_combobox(self.top, "Holiday.TCombobox", self.theme, self.scale)
        self.primary_combo.configure(style="Holiday.TCombobox")
        self.secondary_combo.configure(style="Holiday.TCombobox")
        self.save_button.apply_theme(self.theme)
        self.top.lift()
