/* ═══════════════════════════════════════════════════════════════════
   Aivyx Ops Dashboard — Engine API Client
   ═══════════════════════════════════════════════════════════════════
   All requests go through Nginx reverse proxy at /api/*
   which injects the Bearer token server-side.
   ═══════════════════════════════════════════════════════════════════ */

const API_BASE = '/api';

class AivyxAPI {
    constructor() {
        this._base = API_BASE;
    }

    async _fetch(path, opts = {}) {
        const url = `${this._base}${path}`;
        const res = await fetch(url, {
            headers: { 'Content-Type': 'application/json', ...opts.headers },
            ...opts,
        });
        if (!res.ok) {
            const text = await res.text().catch(() => '');
            throw new Error(`API ${res.status}: ${text || res.statusText}`);
        }
        return res;
    }

    async _json(path, opts) {
        const res = await this._fetch(path, opts);
        return res.json();
    }

    // ── Health ──────────────────────────────────────────────────────
    async health() {
        return this._json('/health');
    }

    // ── System Status ──────────────────────────────────────────────
    async status() {
        return this._json('/status');
    }

    // ── Schedules ──────────────────────────────────────────────────
    async listSchedules() {
        return this._json('/schedules');
    }

    // ── Notifications ──────────────────────────────────────────────
    async listNotifications() {
        return this._json('/notifications');
    }

    async clearNotifications() {
        return this._fetch('/notifications', { method: 'DELETE' });
    }

    // ── Agents ─────────────────────────────────────────────────────
    async listAgents() {
        return this._json('/agents');
    }

    async getAgent(name) {
        return this._json(`/agents/${encodeURIComponent(name)}`);
    }

    // ── Chat (SSE Streaming) ───────────────────────────────────────
    streamChat(agent, message, onChunk, onDone, onError) {
        const url = `${this._base}/chat/stream`;
        const body = JSON.stringify({ agent, message });

        fetch(url, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body,
        }).then(async (res) => {
            if (!res.ok) {
                const text = await res.text().catch(() => '');
                onError(new Error(`Chat ${res.status}: ${text}`));
                return;
            }

            const reader = res.body.getReader();
            const decoder = new TextDecoder();
            let buffer = '';

            while (true) {
                const { done, value } = await reader.read();
                if (done) break;
                buffer += decoder.decode(value, { stream: true });

                const lines = buffer.split('\n');
                buffer = lines.pop() || '';

                for (const line of lines) {
                    if (line.startsWith('data: ')) {
                        const data = line.slice(6);
                        if (data === '[DONE]') {
                            onDone();
                            return;
                        }
                        try {
                            const parsed = JSON.parse(data);
                            if (parsed.type === 'token' && parsed.content) {
                                onChunk(parsed.content);
                            } else if (parsed.type === 'error') {
                                onError(new Error(parsed.message || 'Agent error'));
                                return;
                            }
                        } catch {
                            // Ignore non-JSON lines
                        }
                    }
                }
            }
            onDone();
        }).catch(onError);
    }

    // ── Metrics ────────────────────────────────────────────────────
    async metricsSummary() {
        return this._json('/metrics/summary');
    }

    async metricsTimeline() {
        return this._json('/metrics/timeline');
    }

    // ── Audit ──────────────────────────────────────────────────────
    async recentAudit() {
        return this._json('/audit');
    }

    // ── Notifications (Feedback Loop) ─────────────────────────────
    async rateNotification(id, rating) {
        return this._json(`/notifications/${encodeURIComponent(id)}/rating`, {
            method: 'PUT',
            body: JSON.stringify({ rating }),
        });
    }

    async notificationHistory(params = {}) {
        const qs = new URLSearchParams();
        if (params.agent) qs.set('agent', params.agent);
        if (params.rating) qs.set('rating', params.rating);
        if (params.limit) qs.set('limit', String(params.limit));
        const query = qs.toString();
        return this._json(`/notifications/history${query ? '?' + query : ''}`);
    }
}

// Global singleton
window.api = new AivyxAPI();
