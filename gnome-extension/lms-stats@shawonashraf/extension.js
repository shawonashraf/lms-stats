// lms-stats panel indicator: all-time token total in the top bar; today, this
// week and this month in the popup. Reads the proxy's JSON API over HTTP.

import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import Soup from 'gi://Soup?version=3.0';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

// ponytail: one constant instead of a prefs UI; add GSettings when the proxy moves off this machine.
const PROXY = 'http://127.0.0.1:1235';
const REFRESH_SECONDS = 15;
const PERIODS = [['today', 'Today'], ['week', 'This week'], ['month', 'This month']];

const compact = (n) =>
    n >= 1e9 ? `${(n / 1e9).toFixed(1)}B`
    : n >= 1e6 ? `${(n / 1e6).toFixed(1)}M`
    : n >= 1e3 ? `${(n / 1e3).toFixed(1)}k`
    : `${n}`;

/** Unix seconds at local midnight for today, this week's Monday, and the 1st of this month. */
function periodStarts() {
    const now = new Date();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const monday = new Date(today);
    monday.setDate(today.getDate() - ((today.getDay() + 6) % 7));
    const month = new Date(now.getFullYear(), now.getMonth(), 1);
    return {today: today.getTime() / 1000, week: monday.getTime() / 1000, month: month.getTime() / 1000};
}

const Indicator = GObject.registerClass(
class Indicator extends PanelMenu.Button {
    _init(session) {
        super._init(0.0, 'lms-stats');
        this._session = session;
        this._destroyed = false;

        const box = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        box.add_child(new St.Icon({icon_name: 'utilities-system-monitor-symbolic', style_class: 'system-status-icon'}));
        this._label = new St.Label({text: '—', y_align: Clutter.ActorAlign.CENTER});
        box.add_child(this._label);
        this.add_child(box);

        this._rows = {};
        for (const [key, title] of PERIODS) {
            const item = new PopupMenu.PopupMenuItem(title, {reactive: false});
            const value = new St.Label({text: '—', x_expand: true, x_align: Clutter.ActorAlign.END});
            item.add_child(value);
            this.menu.addMenuItem(item);
            this._rows[key] = value;
        }
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        const open = new PopupMenu.PopupMenuItem('Open dashboard');
        open.connect('activate', () => Gio.AppInfo.launch_default_for_uri(`${PROXY}/dashboard`, null));
        this.menu.addMenuItem(open);

        this.menu.connect('open-state-changed', (_menu, isOpen) => {
            if (isOpen)
                this.refresh();
        });
    }

    destroy() {
        this._destroyed = true;
        super.destroy();
    }

    async refresh() {
        try {
            const starts = periodStarts();
            const [all, month] = await Promise.all([
                this._fetch(`${PROXY}/api/aggregate?bucket=day&from=0`),
                this._fetch(`${PROXY}/api/aggregate?bucket=day&from=${starts.month}`),
            ]);
            if (this._destroyed)
                return;
            this._label.text = compact(all.totals.total);

            // Day buckets are keyed YYYY-MM-DD in the proxy host's local time; the
            // proxy runs on this machine, so compare them as local dates.
            const sums = {today: [0, 0], week: [0, 0], month: [0, 0]};
            for (const b of month.buckets) {
                const [y, m, d] = b.key.split('-').map(Number);
                const ts = new Date(y, m - 1, d).getTime() / 1000;
                for (const [p] of PERIODS) {
                    if (ts >= starts[p]) {
                        sums[p][0] += b.total;
                        sums[p][1] += b.requests;
                    }
                }
            }
            for (const [p] of PERIODS) {
                const [tokens, requests] = sums[p];
                this._rows[p].text = `${tokens.toLocaleString()} tokens, ${requests} ${requests === 1 ? 'request' : 'requests'}`;
            }
        } catch (e) {
            if (this._destroyed)
                return;
            this._label.text = '—';
            for (const [p] of PERIODS)
                this._rows[p].text = 'proxy unreachable';
            console.debug(`lms-stats: ${e}`);
        }
    }

    _fetch(url) {
        return new Promise((resolve, reject) => {
            const msg = Soup.Message.new('GET', url);
            this._session.send_and_read_async(msg, GLib.PRIORITY_DEFAULT, null, (session, res) => {
                try {
                    const bytes = session.send_and_read_finish(res);
                    if (msg.get_status() !== 200)
                        throw new Error(`HTTP ${msg.get_status()}`);
                    resolve(JSON.parse(new TextDecoder().decode(bytes.get_data())));
                } catch (e) {
                    reject(e);
                }
            });
        });
    }
});

export default class LmsStatsExtension extends Extension {
    enable() {
        this._session = new Soup.Session({timeout: 5});
        this._indicator = new Indicator(this._session);
        Main.panel.addToStatusArea(this.uuid, this._indicator);
        this._indicator.refresh();
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, REFRESH_SECONDS, () => {
            this._indicator.refresh();
            return GLib.SOURCE_CONTINUE;
        });
    }

    disable() {
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = null;
        }
        this._indicator?.destroy();
        this._indicator = null;
        this._session?.abort();
        this._session = null;
    }
}
