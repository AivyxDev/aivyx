/* ═══════════════════════════════════════════════════════════════════
   Aivyx Ops Dashboard — Main Application
   ═══════════════════════════════════════════════════════════════════ */

(() => {
    'use strict';

    // ── State ──────────────────────────────────────────────────────
    let scheduleFilter = 'all';
    let chatOpen = false;
    let chatStreaming = false;
    const REFRESH_INTERVAL = 60_000; // 60s

    // ── Init ───────────────────────────────────────────────────────
    document.addEventListener('DOMContentLoaded', () => {
        initClock();
        initFilters();
        initChat();
        initRefresh();
        loadAll();
        setInterval(loadAll, REFRESH_INTERVAL);
    });

    // ── Clock ──────────────────────────────────────────────────────
    function initClock() {
        const el = document.getElementById('clock');
        function tick() {
            const now = new Date();
            el.textContent = now.toLocaleTimeString('en-GB', {
                hour: '2-digit', minute: '2-digit', second: '2-digit',
                timeZone: 'Asia/Singapore'
            });
        }
        tick();
        setInterval(tick, 1000);
    }

    // ── Refresh Button ─────────────────────────────────────────────
    function initRefresh() {
        document.getElementById('refresh-btn').addEventListener('click', () => {
            loadAll();
        });
    }

    // ── Load All Data ──────────────────────────────────────────────
    async function loadAll() {
        await Promise.allSettled([
            loadHealth(),
            loadStatus(),
            loadSchedules(),
            loadNotifications(),
            loadAgents(),
            loadMetrics(),
        ]);
    }

    // ── Health ─────────────────────────────────────────────────────
    async function loadHealth() {
        const badge = document.getElementById('health-badge');
        const dot = badge.querySelector('.health-dot');
        const text = badge.querySelector('.health-text');
        try {
            const data = await api.health();
            dot.className = 'health-dot online';
            text.textContent = data.status === 'ok' ? 'Online' : data.status;
        } catch {
            dot.className = 'health-dot offline';
            text.textContent = 'Offline';
        }
    }

    // ── Status ─────────────────────────────────────────────────────
    async function loadStatus() {
        try {
            const data = await api.status();
            const pills = document.getElementById('stat-pills');
            pills.innerHTML = `
                <div class="stat-pill"><span class="label">Provider</span><span class="value">${esc(data.provider)}</span></div>
                <div class="stat-pill"><span class="label">Agents</span><span class="value">${data.agent_count}</span></div>
                <div class="stat-pill"><span class="label">Sessions</span><span class="value">${data.session_count}</span></div>
                <div class="stat-pill"><span class="label">Audit</span><span class="value">${data.audit_entries}</span></div>
            `;
        } catch { /* silent */ }
    }

    // ── Schedules ──────────────────────────────────────────────────
    async function loadSchedules() {
        try {
            const data = await api.listSchedules();
            const schedules = data.schedules || data || [];
            document.getElementById('schedule-count').textContent = schedules.length;
            renderSchedules(schedules);
        } catch (err) {
            console.error('Failed to load schedules:', err);
        }
    }

    function renderSchedules(schedules) {
        const list = document.getElementById('schedule-list');
        const filtered = scheduleFilter === 'all'
            ? schedules
            : schedules.filter(s => s.agent === scheduleFilter);

        if (filtered.length === 0) {
            list.innerHTML = '<div class="feed-empty"><p>No schedules found.</p></div>';
            return;
        }

        list.innerHTML = filtered.map(s => {
            const agentClass = s.agent === 'business-admin' ? 'biz' : 'ops';
            const next = nextCronFire(s.cron);
            return `
                <div class="schedule-item" data-agent="${esc(s.agent)}">
                    <div class="schedule-indicator ${agentClass}"></div>
                    <div class="schedule-info">
                        <div class="schedule-name">${esc(s.name)}</div>
                        <div class="schedule-meta">
                            <span class="schedule-cron">${esc(s.cron)}</span>
                            <span class="schedule-next">${next}</span>
                        </div>
                    </div>
                    <span class="badge" style="font-size:0.65rem">${s.enabled ? '✓' : '✗'}</span>
                </div>
            `;
        }).join('');
    }

    // ── Notifications ──────────────────────────────────────────────
    let feedTab = 'pending';

    async function loadNotifications() {
        try {
            const data = await api.listNotifications();
            const notifications = data.notifications || data || [];
            document.getElementById('notification-count').textContent = notifications.length;
            renderNotifications(notifications);
        } catch (err) {
            console.error('Failed to load notifications:', err);
        }
    }

    function renderNotifications(notifications) {
        const list = document.getElementById('feed-list');

        if (!notifications || notifications.length === 0) {
            list.innerHTML = `
                <div class="feed-empty">
                    <p>Waiting for agent outputs...</p>
                    <p class="muted">Notifications will appear here as schedules fire.</p>
                </div>
            `;
            return;
        }

        list.innerHTML = notifications.map(n => {
            const source = n.source || n.schedule_name || '';
            const agent = source.startsWith('biz-') ? 'biz' : 'ops';
            const agentLabel = agent === 'biz' ? '🏢 Business Admin' : '⚙️ DevOps';
            const agentClass = `agent-${agent}`;
            const time = (n.created_at || n.timestamp) ? formatTime(n.created_at || n.timestamp) : '';
            const content = n.content || n.result || '';
            const isLong = content.length > 400;
            const nid = n.id || '';

            // Rating badge (if already rated)
            const ratingBadge = n.rating
                ? `<span class="rating-badge rating-${n.rating}">${ratingLabel(n.rating)}</span>`
                : '';

            // Rating buttons (only if not yet rated)
            const ratingButtons = !n.rating && nid
                ? `<div class="rating-buttons" data-id="${esc(nid)}">
                     <button class="rate-btn rate-useful" data-rating="useful" title="Useful">👍</button>
                     <button class="rate-btn rate-partial" data-rating="partial" title="Partial">⚖️</button>
                     <button class="rate-btn rate-useless" data-rating="useless" title="Useless">👎</button>
                   </div>`
                : '';

            return `
                <div class="feed-item ${agentClass}">
                    <div class="feed-item-header">
                        <div>
                            <span class="feed-agent ${agent}">${agentLabel}</span>
                            <span class="feed-schedule">${esc(source)}</span>
                            ${ratingBadge}
                        </div>
                        <span class="feed-time">${time}</span>
                    </div>
                    <div class="feed-content${isLong ? '' : ' expanded'}">${esc(content)}</div>
                    <div class="feed-item-footer">
                        ${isLong ? '<button class="feed-expand" onclick="this.closest(\'.feed-item\').querySelector(\'.feed-content\').classList.toggle(\'expanded\');this.textContent=this.textContent===\'Show more\'?\'Show less\':\'Show more\'">Show more</button>' : ''}
                        ${ratingButtons}
                    </div>
                </div>
            `;
        }).join('');

        // Wire up rating buttons
        list.querySelectorAll('.rate-btn').forEach(btn => {
            btn.addEventListener('click', async (e) => {
                const container = e.target.closest('.rating-buttons');
                const id = container.dataset.id;
                const rating = e.target.dataset.rating;
                try {
                    container.innerHTML = '<span class="rating-loading">Rating...</span>';
                    await api.rateNotification(id, rating);
                    container.innerHTML = `<span class="rating-badge rating-${rating}">${ratingLabel(rating)}</span>`;
                } catch (err) {
                    container.innerHTML = `<span class="rating-error">Failed</span>`;
                    console.error('Rating failed:', err);
                }
            });
        });
    }

    function ratingLabel(rating) {
        switch (rating) {
            case 'useful': return '👍 Useful';
            case 'partial': return '⚖️ Partial';
            case 'useless': return '👎 Useless';
            default: return rating;
        }
    }

    // ── History Tab ────────────────────────────────────────────────
    async function loadHistory() {
        try {
            const data = await api.notificationHistory({ limit: 50 });
            const history = data.notifications || data || [];
            renderHistory(history);
        } catch (err) {
            console.error('Failed to load history:', err);
        }
    }

    function renderHistory(items) {
        const list = document.getElementById('feed-history');

        if (!items || items.length === 0) {
            list.innerHTML = `
                <div class="feed-empty">
                    <p>No rated notifications yet.</p>
                    <p class="muted">Rate outputs using 👍 ⚖️ 👎 to build feedback history.</p>
                </div>
            `;
            return;
        }

        list.innerHTML = items.map(n => {
            const source = n.source || '';
            const agent = source.startsWith('biz-') ? 'biz' : 'ops';
            const agentLabel = agent === 'biz' ? '🏢 Business Admin' : '⚙️ DevOps';
            const time = n.created_at ? formatTime(n.created_at) : '';
            const content = n.content || '';
            const isLong = content.length > 400;
            const ratingBadge = n.rating
                ? `<span class="rating-badge rating-${n.rating}">${ratingLabel(n.rating)}</span>`
                : '';

            return `
                <div class="feed-item agent-${agent}">
                    <div class="feed-item-header">
                        <div>
                            <span class="feed-agent ${agent}">${agentLabel}</span>
                            <span class="feed-schedule">${esc(source)}</span>
                            ${ratingBadge}
                        </div>
                        <span class="feed-time">${time}</span>
                    </div>
                    <div class="feed-content${isLong ? '' : ' expanded'}">${esc(content)}</div>
                    ${isLong ? '<button class="feed-expand" onclick="this.previousElementSibling.classList.toggle(\'expanded\');this.textContent=this.textContent===\'Show more\'?\'Show less\':\'Show more\'">Show more</button>' : ''}
                </div>
            `;
        }).join('');
    }

    // Tab switching
    document.addEventListener('DOMContentLoaded', () => {
        document.querySelectorAll('.tab-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                feedTab = btn.dataset.tab;

                const pending = document.getElementById('feed-list');
                const history = document.getElementById('feed-history');
                const clearBtn = document.getElementById('clear-feed-btn');

                if (feedTab === 'history') {
                    pending.style.display = 'none';
                    history.style.display = 'block';
                    clearBtn.style.display = 'none';
                    loadHistory();
                } else {
                    pending.style.display = 'block';
                    history.style.display = 'none';
                    clearBtn.style.display = '';
                }
            });
        });
    });

    // Clear notifications button
    document.addEventListener('DOMContentLoaded', () => {
        document.getElementById('clear-feed-btn')?.addEventListener('click', async () => {
            try {
                await api.clearNotifications();
                loadNotifications();
            } catch (err) {
                console.error('Failed to clear:', err);
            }
        });
    });

    // ── Agents ─────────────────────────────────────────────────────
    async function loadAgents() {
        try {
            const data = await api.listAgents();
            const agents = data.agents || data || [];
            renderAgents(agents);
        } catch (err) {
            console.error('Failed to load agents:', err);
        }
    }

    function renderAgents(agents) {
        const container = document.getElementById('agent-cards');

        // Filter to show only business-admin and devops
        const opsAgents = agents.filter(a =>
            a.name === 'business-admin' || a.name === 'devops'
        );

        if (opsAgents.length === 0) {
            container.innerHTML = '<div class="feed-empty"><p>No ops agents found.</p></div>';
            return;
        }

        container.innerHTML = opsAgents.map(a => {
            const cls = a.name === 'business-admin' ? 'biz' : 'ops';
            const icon = cls === 'biz' ? '🏢' : '⚙️';
            const toolCount = a.tool_ids?.length || a.tools?.length || 0;
            const soul = a.soul || '';
            const soulExcerpt = soul.split('\n').filter(l => l.trim()).slice(0, 3).join(' ');

            return `
                <div class="agent-card ${cls}">
                    <div class="agent-card-header">
                        <span class="agent-name">${icon} ${esc(a.name)}</span>
                        <span class="agent-role">${esc(a.role || 'agent')}</span>
                    </div>
                    <div class="agent-soul">${esc(soulExcerpt)}</div>
                    <div class="agent-stats">
                        <span class="agent-stat">🔧 <span class="num">${toolCount}</span> tools</span>
                        <span class="agent-stat">🛡️ <span class="num">${esc(a.autonomy_tier || 'Trust')}</span></span>
                    </div>
                </div>
            `;
        }).join('');
    }

    // ── Metrics ────────────────────────────────────────────────────
    async function loadMetrics() {
        try {
            const data = await api.metricsSummary();
            renderMetrics(data);
        } catch {
            renderMetricsFallback();
        }
    }

    function renderMetrics(data) {
        const grid = document.getElementById('metrics-grid');
        const totalTokens = (data.total_input_tokens || 0) + (data.total_output_tokens || 0);
        const cost = data.total_cost_usd || 0;
        const calls = data.total_calls || 0;
        const avgMs = data.avg_latency_ms || 0;

        grid.innerHTML = `
            <div class="metric-card">
                <div class="metric-value">${formatNumber(totalTokens)}</div>
                <div class="metric-label">Total Tokens</div>
            </div>
            <div class="metric-card">
                <div class="metric-value">$${cost.toFixed(2)}</div>
                <div class="metric-label">Total Cost</div>
            </div>
            <div class="metric-card">
                <div class="metric-value">${calls}</div>
                <div class="metric-label">LLM Calls</div>
            </div>
            <div class="metric-card">
                <div class="metric-value">${avgMs > 0 ? (avgMs / 1000).toFixed(1) + 's' : '—'}</div>
                <div class="metric-label">Avg Latency</div>
            </div>
        `;
    }

    function renderMetricsFallback() {
        const grid = document.getElementById('metrics-grid');
        grid.innerHTML = `
            <div class="metric-card"><div class="metric-value">—</div><div class="metric-label">Total Tokens</div></div>
            <div class="metric-card"><div class="metric-value">—</div><div class="metric-label">Total Cost</div></div>
            <div class="metric-card"><div class="metric-value">—</div><div class="metric-label">LLM Calls</div></div>
            <div class="metric-card"><div class="metric-value">—</div><div class="metric-label">Avg Latency</div></div>
        `;
    }

    // ── Filters ────────────────────────────────────────────────────
    function initFilters() {
        document.querySelectorAll('.filter-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                document.querySelectorAll('.filter-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                scheduleFilter = btn.dataset.filter;
                loadSchedules();
            });
        });
    }

    // ── Chat ───────────────────────────────────────────────────────
    function initChat() {
        const toggle = document.getElementById('chat-toggle');
        const container = document.getElementById('chat-container');
        const close = document.getElementById('chat-close');
        const send = document.getElementById('chat-send');
        const input = document.getElementById('chat-input');

        toggle.addEventListener('click', () => {
            chatOpen = !chatOpen;
            container.classList.toggle('open', chatOpen);
        });

        close.addEventListener('click', () => {
            chatOpen = false;
            container.classList.remove('open');
        });

        send.addEventListener('click', sendChatMessage);
        input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                sendChatMessage();
            }
        });

        // Auto-resize textarea
        input.addEventListener('input', () => {
            input.style.height = 'auto';
            input.style.height = Math.min(input.scrollHeight, 120) + 'px';
        });
    }

    function sendChatMessage() {
        if (chatStreaming) return;
        const input = document.getElementById('chat-input');
        const messages = document.getElementById('chat-messages');
        const agent = document.getElementById('chat-agent-select').value;
        const prompt = input.value.trim();

        if (!prompt) return;
        input.value = '';
        input.style.height = 'auto';

        // Clear welcome message
        const welcome = messages.querySelector('.chat-welcome');
        if (welcome) welcome.remove();

        // Add user message
        const userMsg = document.createElement('div');
        userMsg.className = 'chat-msg user';
        userMsg.textContent = prompt;
        messages.appendChild(userMsg);

        // Add assistant message (streaming)
        const assistantMsg = document.createElement('div');
        assistantMsg.className = 'chat-msg assistant streaming';
        messages.appendChild(assistantMsg);
        messages.scrollTop = messages.scrollHeight;

        chatStreaming = true;
        let content = '';

        api.streamChat(
            agent,
            prompt,
            (chunk) => {
                content += chunk;
                assistantMsg.textContent = content;
                messages.scrollTop = messages.scrollHeight;
            },
            () => {
                assistantMsg.classList.remove('streaming');
                chatStreaming = false;
                messages.scrollTop = messages.scrollHeight;
            },
            (err) => {
                assistantMsg.classList.remove('streaming');
                assistantMsg.textContent = `Error: ${err.message}`;
                assistantMsg.style.color = 'var(--error)';
                chatStreaming = false;
            }
        );
    }

    // ── Helpers ─────────────────────────────────────────────────────
    function esc(str) {
        const div = document.createElement('div');
        div.textContent = String(str || '');
        return div.innerHTML;
    }

    function formatNumber(n) {
        if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
        if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
        return String(n);
    }

    function formatTime(ts) {
        try {
            const d = new Date(ts);
            return d.toLocaleString('en-GB', {
                month: 'short', day: 'numeric',
                hour: '2-digit', minute: '2-digit',
                timeZone: 'Asia/Singapore',
            });
        } catch {
            return ts;
        }
    }

    function nextCronFire(cron) {
        // Simple human-readable approximation
        if (!cron) return '';
        const parts = cron.split(' ');
        if (parts.length < 5) return '';

        const [min, hour, dom, mon, dow] = parts;

        if (min.startsWith('*/')) return `every ${min.slice(2)} min`;
        if (hour.startsWith('*/')) return `every ${hour.slice(2)}h`;
        if (dow === '*' && dom === '*') return `daily ${padTime(hour, min)}`;
        if (dow === '1-5') return `weekdays ${padTime(hour, min)}`;
        if (dow === '1') return `Mon ${padTime(hour, min)}`;
        if (dow === '0' || dow === '7') return `Sun ${padTime(hour, min)}`;
        if (dow === '6') return `Sat ${padTime(hour, min)}`;
        return `${padTime(hour, min)} UTC`;
    }

    function padTime(h, m) {
        // Convert UTC to UTC+8 for display
        let hour = parseInt(h, 10);
        if (isNaN(hour)) return `${h}:${m}`;
        hour = (hour + 8) % 24;
        const mm = m.padStart(2, '0');
        return `${String(hour).padStart(2, '0')}:${mm}`;
    }
})();
