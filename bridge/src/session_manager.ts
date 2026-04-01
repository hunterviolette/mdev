import { chromium, type Browser, type BrowserContext, type Locator, type Page } from 'playwright';
import { randomUUID } from 'node:crypto';
import type { ClosePageCommand, ConnectOverCdpCommand, GetCookiesCommand, OpenPageCommand, ProbePageCommand, SendChatCommand, StartSessionCommand, UploadFileCommand, ReadResponseCommand, SetPollConfigCommand, SetResponseTimeoutCommand } from './protocol.js';
import { resolveInteractionProfile } from './profiles.js';

type ResponseSnapshot = {
  texts: string[];
  count: number;
  lastText: string;
};

export type SessionState = {
  sessionId: string;
  browser?: Browser;
  context: BrowserContext;
  page: Page;
  profile?: string;
  responseSelector: string;
  uploadInputSelector?: string;
  uploadButtonSelector?: string;
  chatInputSelector?: string;
  chatSubmitSelector?: string;
  ownsBrowser: boolean;
  attachedViaCdp: boolean;
  responseTimeoutMs?: number;
  responsePollMs: number;
  domPollMs: number;
  pendingUploads: string[];
};

export class SessionManager {
  private sessions = new Map<string, SessionState>();

  async startSession(cmd: StartSessionCommand) {
    const sessionId = cmd.session_id ?? randomUUID();
    const timeout = cmd.timeout_ms ?? 60000;

    const launchOptions: Parameters<typeof chromium.launch>[0] = {
      headless: !(cmd.headed ?? false)
    };

    if (cmd.browser_channel && cmd.browser_channel !== 'chromium') {
      launchOptions.channel = cmd.browser_channel;
    }

    const browser = await chromium.launch(launchOptions);
    const context = await browser.newContext();
    const page = await context.newPage();
    page.setDefaultTimeout(timeout);
    page.setDefaultNavigationTimeout(timeout);

    if (cmd.url && cmd.url.trim()) {
      await page.goto(cmd.url, { waitUntil: 'domcontentloaded', timeout });
    }

    if (cmd.wait_for) {
      await page.waitForSelector(cmd.wait_for, { timeout });
    }

    const { name: resolvedProfileName, profile } = await resolveInteractionProfile(page, cmd.profile ?? 'auto');

    const state: SessionState = {
      sessionId,
      browser,
      context,
      page,
      profile: resolvedProfileName,
      responseSelector: profile.responseSelector,
      uploadInputSelector: profile.uploadInputSelector,
      uploadButtonSelector: profile.uploadButtonSelector,
      chatInputSelector: profile.chatInputSelector,
      chatSubmitSelector: profile.chatSubmitSelector,
      ownsBrowser: true,
      attachedViaCdp: false,
      responseTimeoutMs: undefined,
      responsePollMs: 1000,
      domPollMs: 1000,
      pendingUploads: []
    };

    this.sessions.set(sessionId, state);

    let currentUrl = '';
    try {
      currentUrl = page.url();
    } catch {
      currentUrl = cmd.url ?? '';
    }

    let currentTitle = '';
    try {
      currentTitle = await page.title();
    } catch {
      currentTitle = '';
    }

    return {
      session_id: sessionId,
      attached_via_cdp: false,
      url: currentUrl,
      title: currentTitle
    };
  }

  async connectOverCdp(cmd: ConnectOverCdpCommand) {
    const sessionId = cmd.session_id ?? randomUUID();
    const timeout = cmd.timeout_ms ?? 60000;
    const browser = await chromium.connectOverCDP(cmd.cdp_url, { timeout });

    let context = browser.contexts()[0];
    if (!context) {
      throw new Error('No existing browser context available over CDP');
    }

    let page: Page | undefined;
    const pageUrlContains = cmd.page_url_contains;

    if (pageUrlContains) {
      for (const candidateContext of browser.contexts()) {
        const candidatePage = candidateContext.pages().find((p) => p.url().includes(pageUrlContains));
        if (candidatePage) {
          context = candidateContext;
          page = candidatePage;
          break;
        }
      }

      if (!page) {
        throw new Error(`No existing tab matched page_url_contains: ${pageUrlContains}`);
      }
    } else {
      const pages = context.pages();
      page = pages[0];
      if (!page) {
        throw new Error('No existing page available in persistent browser context');
      }
    }

    page.setDefaultTimeout(timeout);
    page.setDefaultNavigationTimeout(timeout);

    if (cmd.wait_for) {
      await page.waitForSelector(cmd.wait_for, { timeout });
    }

    const { name: resolvedProfileName, profile } = await resolveInteractionProfile(page, cmd.profile ?? 'auto');

    const state: SessionState = {
      sessionId,
      browser,
      context,
      page,
      profile: resolvedProfileName,
      responseSelector: profile.responseSelector,
      uploadInputSelector: profile.uploadInputSelector,
      uploadButtonSelector: profile.uploadButtonSelector,
      chatInputSelector: profile.chatInputSelector,
      chatSubmitSelector: profile.chatSubmitSelector,
      ownsBrowser: false,
      attachedViaCdp: true,
      responseTimeoutMs: undefined,
      responsePollMs: 1000,
      domPollMs: 1000,
      pendingUploads: []
    };

    this.sessions.set(sessionId, state);

    return {
      session_id: sessionId,
      attached_via_cdp: true,
      url: page.url(),
      title: await page.title()
    };
  }

  async openPage(cmd: OpenPageCommand) {
    const state = this.getSession(cmd.session_id);
    const timeout = cmd.timeout_ms ?? 60000;
    const page = await state.context.newPage();
    page.setDefaultTimeout(timeout);
    page.setDefaultNavigationTimeout(timeout);
    await page.goto(cmd.url, { waitUntil: 'domcontentloaded', timeout });
    state.page = page;
    let title = '';
    try {
      title = await page.title();
    } catch {
    }

    return {
      session_id: state.sessionId,
      url: page.url(),
      title
    };
  }

  async probePage(cmd: ProbePageCommand) {
    const state = this.getSession(cmd.session_id);
    const timeout = cmd.timeout_ms ?? 60000;
    const page = state.page;
    const { name: resolvedProfileName, profile } = await resolveInteractionProfile(page, cmd.profile ?? state.profile ?? 'auto');


    state.profile = resolvedProfileName;
    state.responseSelector = profile.responseSelector;
    state.uploadInputSelector = profile.uploadInputSelector;
    state.uploadButtonSelector = profile.uploadButtonSelector;
    state.chatInputSelector = profile.chatInputSelector;
    state.chatSubmitSelector = profile.chatSubmitSelector;

    if (page.isClosed()) {
      return {
        session_id: state.sessionId,
        browser_connected: true,
        page_open: false,
        url: '',
        profile: state.profile ?? 'auto',
        chat_input_found: false,
        chat_input_visible: false,
        chat_submit_found: false,
        ready: false
      };
    }

    const chatInputSelector = state.chatInputSelector ?? profile.chatInputSelector;
    if (!chatInputSelector) {
      return {
        session_id: state.sessionId,
        browser_connected: true,
        page_open: true,
        url: page.url(),
        profile: state.profile ?? 'auto',
        chat_input_found: false,
        chat_input_visible: false,
        chat_submit_found: false,
        ready: false
      };
    }

    const directChatInput = page.locator(chatInputSelector).first();
    const directInputCount = await directChatInput.count().catch(() => 0);
    const resolvedComposer = await this.findVisibleChatComposer(page, Math.min(timeout, 1000), chatInputSelector, state.domPollMs).catch(() => null);
    const resolvedSubmit = resolvedComposer
      ? await this.findActionableSubmitButton(page, resolvedComposer, state.chatSubmitSelector ?? profile.chatSubmitSelector).catch(() => null)
      : null;

    const chatInputFound = directInputCount > 0 || resolvedComposer !== null;
    const chatInputVisible = resolvedComposer !== null;
    const chatSubmitFound = resolvedSubmit !== null;

    return {
      session_id: state.sessionId,
      browser_connected: true,
      page_open: true,
      url: page.url(),
      profile: state.profile ?? 'auto',
      chat_input_found: chatInputFound,
      chat_input_visible: chatInputVisible,
      chat_submit_found: chatSubmitFound,
      ready: chatInputVisible
    };
  }

  private async waitForSendReady(page: Page, composer: Locator, submitSelector: string | undefined, timeoutMs: number): Promise<Locator | null> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const submit = await this.findActionableSubmitButton(page, composer, submitSelector).catch(() => null);
      if (submit) {
        return submit;
      }
      await page.waitForTimeout(250);
    }
    return null;
  }

  private async findActionableSubmitButton(page: Page, composer: Locator, preferredSelector?: string): Promise<Locator | null> {
    const composerBox = await composer.boundingBox().catch(() => null);

    const selectors = [
      preferredSelector,
      'button[data-testid="send-button"]',
      'button[aria-label*="send" i]',
      'button[type="submit"]',
      'form button',
      'button'
    ].filter((selector): selector is string => Boolean(selector));

    let best: { locator: Locator; score: number } | null = null;

    for (const selector of selectors) {
      const locator = page.locator(selector);
      const count = await locator.count().catch(() => 0);
      for (let i = 0; i < count; i += 1) {
        const candidate = locator.nth(i);
        const visible = await candidate.isVisible().catch(() => false);
        if (!visible) {
          continue;
        }

        const enabled = await candidate.isEnabled().catch(() => false);
        if (!enabled) {
          continue;
        }

        const meta = await candidate.evaluate((el) => {
          const aria = (el.getAttribute('aria-label') ?? '').toLowerCase();
          const title = (el.getAttribute('title') ?? '').toLowerCase();
          const text = (el.textContent ?? '').trim().toLowerCase();
          const hasVectorIcon = el.querySelector('svg, path, circle') !== null;
          const disabled = el.hasAttribute('disabled') || el.getAttribute('aria-disabled') === 'true';
          return { aria, title, text, hasVectorIcon, disabled };
        }).catch(() => null);

        if (!meta || meta.disabled) {
          continue;
        }

        if (selector === 'button') {
          const hasSendSemantics = meta.aria.includes('send')
            || meta.title.includes('send')
            || meta.text === 'send'
            || meta.hasVectorIcon;
          if (!hasSendSemantics) {
            continue;
          }
        }

        const box = await candidate.boundingBox().catch(() => null);
        if (!box) {
          continue;
        }

        let score = 0;
        if (preferredSelector && selector === preferredSelector) {
          score += 1000;
        }
        if (selector === 'button[data-testid="send-button"]') {
          score += 500;
        }
        if (selector === 'button[aria-label*="send" i]') {
          score += 400;
        }
        if (selector === 'button[type="submit"]') {
          score += 300;
        }
        if (meta.aria.includes('send') || meta.title.includes('send') || meta.text === 'send') {
          score += 200;
        }

        if (composerBox) {
          const dx = Math.min(
            Math.abs(box.x - (composerBox.x + composerBox.width)),
            Math.abs((box.x + box.width) - (composerBox.x + composerBox.width)),
            Math.abs(box.x - composerBox.x)
          );
          const dy = Math.min(
            Math.abs(box.y - composerBox.y),
            Math.abs(box.y - (composerBox.y + composerBox.height)),
            Math.abs((box.y + box.height) - (composerBox.y + composerBox.height))
          );
          score -= Math.trunc(dx + dy);
        }

        if (!best || score > best.score) {
          best = { locator: candidate, score };
        }
      }
    }

    return best?.locator ?? null;
  }

  async closePage(cmd: ClosePageCommand) {
    const state = this.getSession(cmd.session_id);
    const page = state.page;

    if (!page.isClosed()) {
      await page.close();
    }

    const remainingPages = state.context.pages().filter((p) => !p.isClosed());
    state.page = remainingPages[0] ?? await state.context.newPage();

    return {
      session_id: state.sessionId,
      page_closed: true,
      remaining_pages: remainingPages.length,
      url: state.page.url()
    };
  }

  private async findVisibleChatComposer(page: Page, timeoutMs: number, preferredSelector?: string, domPollMs = 1000): Promise<Locator> {
    const deadline = Date.now() + timeoutMs;
    const selectors = [
      preferredSelector,
      'div[contenteditable="true"]:visible',
      '[role="textbox"]:visible',
      'textarea:visible'
    ].filter((selector): selector is string => Boolean(selector));

    let lastError: unknown = null;

    while (Date.now() < deadline) {
      for (const selector of selectors) {
        try {
          const locator = page.locator(selector).first();
          const count = await locator.count();
          if (count === 0) {
            continue;
          }
          if (!(await locator.isVisible().catch(() => false))) {
            continue;
          }

          const info = await locator.evaluate((el) => ({
            tagName: el.tagName.toLowerCase(),
            className: ((el as HTMLElement).className ?? '').toString(),
            isContentEditable: (el as HTMLElement).isContentEditable
          }));

          if (info.tagName === 'textarea' && info.className.includes('fallbackTextarea')) {
            continue;
          }

          return locator;
        } catch (err) {
          lastError = err;
        }
      }

      await page.waitForTimeout(Math.max(250, Math.trunc(domPollMs || 1000)));
    }

    if (lastError) {
      throw lastError;
    }

    throw new Error('No visible chat composer found');
  }

  private async writeChatComposerText(page: Page, composer: Locator, text: string): Promise<void> {
    const info = await composer.evaluate((el) => ({
      tagName: el.tagName.toLowerCase(),
      isContentEditable: (el as HTMLElement).isContentEditable
    }));

    await composer.click({ timeout: 5000 });

    if (info.isContentEditable) {
      await composer.evaluate((el) => {
        el.focus();
        el.textContent = '';
        const root = el;
        root.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward', data: null }));
      });
      await page.keyboard.insertText(text);
      return;
    }

    if (info.tagName === 'textarea' || info.tagName === 'input') {
      await composer.fill(text);
      return;
    }

    await page.keyboard.press(process.platform === 'darwin' ? 'Meta+A' : 'Control+A').catch(() => {});
    await page.keyboard.press('Backspace').catch(() => {});
    await page.keyboard.insertText(text);
  }

  async sendChat(cmd: SendChatCommand) {
    const state = this.getSession(cmd.session_id);
    state.responseTimeoutMs = undefined;
    const timeout = Math.max(cmd.timeout_ms ?? 60000, 60000);
    const inputSelector = cmd.input_selector ?? state.chatInputSelector;
    const submitSelector = cmd.submit_selector ?? state.chatSubmitSelector;

    if (state.pendingUploads.length > 0) {
      const uploadReady = await this.waitForPendingUploads(state, Math.min(timeout, 45000));
      if (!uploadReady) {
        throw new Error(`Upload did not finish before send_chat for session ${state.sessionId}: ${state.pendingUploads.join(', ')}`);
      }
    }

    const composer = await this.findVisibleChatComposer(state.page, timeout, inputSelector, state.domPollMs);

    await this.writeChatComposerText(state.page, composer, cmd.text);

    const beforeSendText = await this.readComposerText(composer);
    if (beforeSendText.length === 0) {
      return {
        session_id: state.sessionId,
        sent: false,
        method: 'none',
        text: cmd.text
      };
    }

    const sendResult = await this.trySendUntilDeadline(state.page, composer, submitSelector, timeout);
    if (!sendResult.sent) {
      throw new Error(`Chat submit did not clear composer within ${timeout}ms after ${sendResult.attempts} attempts for session ${state.sessionId}`);
    }

    state.pendingUploads = [];
    return {
      session_id: state.sessionId,
      sent: true,
      method: sendResult.method,
      attempts: sendResult.attempts,
      text: cmd.text
    };
  }

  private async waitForComposerTextChange(composer: Locator, previousText: string, timeoutMs: number): Promise<boolean> {
    const deadline = Date.now() + timeoutMs;

    while (Date.now() < deadline) {
      const text = await this.readComposerText(composer).catch(() => '');
      if (text !== previousText) {
        return true;
      }

      await composer.page().waitForTimeout(100);
    }

    return false;
  }

  private async waitForPostClickSendResult(composer: Locator, beforeClickText: string, timeoutMs: number): Promise<'cleared' | 'changed' | 'unchanged'> {
    const cleared = await this.waitForComposerToClear(composer, Math.min(timeoutMs, 2500)).catch(() => false);
    if (cleared) {
      return 'cleared';
    }

    const changed = await this.waitForComposerTextChange(composer, beforeClickText, Math.min(timeoutMs, 1500)).catch(() => false);
    if (changed) {
      return 'changed';
    }

    return 'unchanged';
  }

  private async trySendUntilDeadline(page: Page, composer: Locator, submitSelector: string | undefined, timeoutMs: number): Promise<{ sent: boolean; method: 'click'; attempts: number }> {
    const deadline = Date.now() + timeoutMs;
    let attempts = 0;
    let lastText = await this.readComposerText(composer).catch(() => '');

    while (Date.now() < deadline) {
      const currentText = await this.readComposerText(composer).catch(() => '');
      if (currentText.length === 0) {
        return { sent: true, method: 'click', attempts };
      }
      lastText = currentText;

      const remainingMs = deadline - Date.now();
      if (remainingMs <= 0) {
        break;
      }

      const submit = await this.waitForSendReady(page, composer, submitSelector, Math.min(remainingMs, 5000));
      if (!submit) {
        await page.waitForTimeout(500).catch(() => {});
        continue;
      }

      attempts += 1;
      await submit.click({ timeout: Math.min(remainingMs, 5000) });

      const postClick = await this.waitForPostClickSendResult(composer, lastText, Math.min(deadline - Date.now(), 5000));
      if (postClick === 'cleared') {
        return { sent: true, method: 'click', attempts };
      }

      const afterClickText = await this.readComposerText(composer).catch(() => '');
      if (afterClickText.length === 0) {
        return { sent: true, method: 'click', attempts };
      }

      await page.waitForTimeout(750).catch(() => {});
    }

    return { sent: false, method: 'click', attempts };
  }

  private getSessionPollMsHint(_page: Page): number {
    return 1000;
  }

  private async readComposerText(composer: Locator): Promise<string> {
    return await composer.evaluate((el) => {
      if ((el as HTMLElement).isContentEditable) {
        return (el.textContent ?? '').trim();
      }
      if (el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement) {
        return el.value.trim();
      }
      return (el.textContent ?? '').trim();
    }).catch(() => '');
  }

  async uploadFile(cmd: UploadFileCommand) {
    const state = this.getSession(cmd.session_id);
    const timeout = cmd.timeout_ms ?? 60000;
    const inputSelector = cmd.input_selector ?? state.uploadInputSelector ?? 'input[type="file"]';
    const uploadName = this.basename(cmd.file_path);
    let via = 'input';

    const input = state.page.locator(inputSelector).first();
    if (await input.count()) {
      await input.setInputFiles(cmd.file_path, { timeout });
    } else {
      const triggerSelector = cmd.button_selector ?? state.uploadButtonSelector;
      if (!triggerSelector) {
        throw new Error(`No file input found for session ${state.sessionId}`);
      }

      const trigger = state.page.locator(triggerSelector).first();
      const chooserPromise = state.page.waitForEvent('filechooser', { timeout });
      await trigger.click({ timeout });
      const chooser = await chooserPromise;
      await chooser.setFiles(cmd.file_path);
      via = 'filechooser';
    }

    state.pendingUploads = Array.from(new Set([...state.pendingUploads, uploadName]));
    const ready = await this.waitForPendingUploads(state, Math.min(timeout, 20000));
    if (!ready) {
      throw new Error(`Upload did not become ready for session ${state.sessionId}: ${uploadName}`);
    }

    return { session_id: state.sessionId, uploaded: cmd.file_path, upload_name: uploadName, ready: true, via };
  }

  private async readResponseSnapshot(page: Page, selector: string): Promise<ResponseSnapshot> {
    const snap = await page.locator(selector).evaluateAll((nodes) => {
      const visibleNodes = nodes.filter((node) => {
        const el = node as HTMLElement;
        const style = window.getComputedStyle(el);
        if (style.display === 'none' || style.visibility === 'hidden') {
          return false;
        }
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });

      const texts = visibleNodes
        .map((node) => (node.textContent ?? '').trim())
        .filter((text) => text.length > 0);

      return {
        texts,
        count: texts.length,
        lastText: texts.length ? texts[texts.length - 1] : ''
      };
    }).catch(() => ({ texts: [], count: 0, lastText: '' }));

    return snap;
  }

  private async waitForCompletedResponse(
    state: SessionState,
    selector: string,
    timeoutMs: number
  ): Promise<ResponseSnapshot> {
    const start = Date.now();
    const effectiveTimeoutMs = Math.max(timeoutMs, 1000);

    while (true) {
      if (Date.now() - start >= effectiveTimeoutMs) {
        throw new Error(`Timed out waiting for completed response after ${effectiveTimeoutMs} ms`);
      }

      const ready = await state.page.evaluate(() => ({
        ready: !document.querySelector('button[aria-label*="Stop" i]') &&
               !!document.querySelector('div[contenteditable="true"], textarea, [role="textbox"]')
      })).catch(() => ({ ready: false }));

      const snap = await this.readResponseSnapshot(state.page, selector).catch(() => ({ texts: [], count: 0, lastText: '' }));
      const hasResponse = snap.count > 0 && snap.lastText.length > 0;
      const done = hasResponse && ready.ready;

      if (done) {
        state.responseTimeoutMs = undefined;
        return snap;
      }

      await state.page.waitForTimeout(Math.max(250, Math.trunc(state.responsePollMs ?? 1000)));
    }
  }

  setPollConfig(cmd: SetPollConfigCommand) {
    const state = this.getSession(cmd.session_id);
    state.responsePollMs = Math.max(250, Math.trunc(cmd.response_poll_ms || state.responsePollMs || 1000));
    state.domPollMs = Math.max(250, Math.trunc(cmd.dom_poll_ms || state.domPollMs || 1000));
    return {
      session_id: state.sessionId,
      response_poll_ms: state.responsePollMs,
      dom_poll_ms: state.domPollMs
    };
  }

  setResponseTimeout(cmd: SetResponseTimeoutCommand) {
    const state = this.getSession(cmd.session_id);
    const prev = state.responseTimeoutMs ?? null;
    const next = Math.max(1000, Math.trunc(cmd.timeout_ms || 0));
    state.responseTimeoutMs = next;
    console.error(`[bridge] setResponseTimeout sessionId=${state.sessionId} previousTimeoutMs=${prev} timeoutMs=${state.responseTimeoutMs}`);
    return {
      session_id: state.sessionId,
      timeout_ms: state.responseTimeoutMs
    };
  }

  async readResponse(cmd: ReadResponseCommand) {
    const state = this.getSession(cmd.session_id);
    const timeout = cmd.timeout_ms ?? 120000;
    const selector = cmd.response_selector ?? state.responseSelector;

    const snap = await this.waitForCompletedResponse(
      state,
      selector,
      timeout
    );

    return {
      session_id: state.sessionId,
      response: snap.lastText,
      response_count: snap.count
    };
  }

  private buildCookieHeader(cookies: Awaited<ReturnType<BrowserContext['cookies']>>, urls?: string[]) {
    const nowSeconds = Math.trunc(Date.now() / 1000);
    const preferredUrl = urls && urls.length > 0 ? urls[0] : undefined;
    const preferred = preferredUrl ? new URL(preferredUrl) : null;
    const wantedCookieNames = [/^MYSAPSSO2$/i, /^SAP_SESSIONID_/i, /^sap-usercontext$/i];

    const filtered = cookies.filter((cookie) => {
      if (!cookie.name || !cookie.value) {
        return false;
      }
      if (cookie.expires > 0 && cookie.expires <= nowSeconds) {
        return false;
      }
      if (!wantedCookieNames.some((rx) => rx.test(cookie.name))) {
        return false;
      }
      return true;
    });

    const scored = filtered.map((cookie, index) => {
      let score = 0;

      if (preferred) {
        const normalizedDomain = cookie.domain.replace(/^\./, '');
        if (preferred.hostname === normalizedDomain) {
          score += 8;
        } else if (preferred.hostname.endsWith(`.${normalizedDomain}`)) {
          score += 4;
        }

        if (preferred.pathname.startsWith(cookie.path || '/')) {
          score += Math.min((cookie.path || '/').length, 32);
        }

        if ((preferred.protocol === 'https:' && cookie.secure) || !cookie.secure) {
          score += 2;
        }
      }

      if (cookie.httpOnly) {
        score += 1;
      }

      return { cookie, index, score };
    });

    const byName = new Map<string, { cookie: (typeof scored)[number]['cookie']; index: number; score: number }>();
    for (const entry of scored) {
      const existing = byName.get(entry.cookie.name);
      if (!existing || entry.score > existing.score || (entry.score === existing.score && entry.index < existing.index)) {
        byName.set(entry.cookie.name, entry);
      }
    }

    const selected = [...byName.values()]
      .sort((a, b) => a.index - b.index)
      .map((entry) => entry.cookie);

    return {
      cookies: selected,
      cookie_names: selected.map((cookie) => cookie.name),
      cookie_header: selected.map((cookie) => `${cookie.name}=${cookie.value}`).join('; ')
    };
  }

  async getCookies(cmd: GetCookiesCommand) {
    const state = this.getSession(cmd.session_id);
    const rawCookies = cmd.urls && cmd.urls.length > 0
      ? await state.context.cookies(cmd.urls)
      : await state.context.cookies();
    const cookieData = this.buildCookieHeader(rawCookies, cmd.urls);

    return {
      session_id: state.sessionId,
      cookies: cookieData.cookies,
      cookie_names: cookieData.cookie_names,
      cookie_header: cookieData.cookie_header
    };
  }

  async closeSession(sessionId: string) {
    const state = this.getSession(sessionId);
    this.sessions.delete(sessionId);

    if (state.ownsBrowser) {
      if (state.browser) {
        await state.browser.close();
      } else {
        await state.context.close();
      }
    }

    return { session_id: sessionId, closed: true, detached_only: state.attachedViaCdp };
  }

  async closeAllSessions() {
    const sessionIds = [...this.sessions.keys()];
    const results = [];

    for (const sessionId of sessionIds) {
      try {
        const result = await this.closeSession(sessionId);
        results.push(result);
      } catch (err) {
        results.push({
          session_id: sessionId,
          closed: false,
          error: err instanceof Error ? err.message : String(err)
        });
      }
    }

    return {
      closed_count: results.filter((item) => item.closed).length,
      total_count: results.length,
      results
    };
  }

  private basename(filePath: string): string {
    const normalized = filePath.replace(/\\/g, '/');
    const parts = normalized.split('/').filter(Boolean);
    return parts.length > 0 ? parts[parts.length - 1] : filePath;
  }

  private async hasVisibleText(page: Page, text: string): Promise<boolean> {
    const trimmed = text.trim();
    if (!trimmed) {
      return false;
    }

    const escaped = trimmed.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
    const exact = page.getByText(trimmed, { exact: true }).first();
    if (await exact.isVisible().catch(() => false)) {
      return true;
    }

    const locator = page.locator(`text=${escaped}`).first();
    return await locator.isVisible().catch(() => false);
  }

  private async waitForPendingUploads(state: SessionState, timeoutMs: number): Promise<boolean> {
    if (state.pendingUploads.length === 0) {
      return true;
    }

    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const allVisible = await Promise.all(state.pendingUploads.map((name) => this.hasVisibleText(state.page, name)));
      if (allVisible.every(Boolean)) {
        return true;
      }
      await state.page.waitForTimeout(250);
    }

    return false;
  }

  private getSession(sessionId: string): SessionState {
    const state = this.sessions.get(sessionId);
    if (!state) {
      throw new Error(`Unknown session_id: ${sessionId}`);
    }
    return state;
  }

  private async waitForComposerToClear(composer: Locator, timeoutMs: number): Promise<boolean> {
    const deadline = Date.now() + timeoutMs;

    while (Date.now() < deadline) {
      const text = await this.readComposerText(composer).catch(() => '');
      if (text.length === 0) {
        return true;
      }

      await composer.page().waitForTimeout(100);
    }

    return false;
  }
}
