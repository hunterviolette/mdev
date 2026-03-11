import { chromium, type Browser, type BrowserContext, type Locator, type Page } from 'playwright';
import { randomUUID } from 'node:crypto';
import type { ClosePageCommand, ConnectOverCdpCommand, OpenPageCommand, ProbePageCommand, SendChatCommand, StartSessionCommand, UploadFileCommand, ReadResponseCommand } from './protocol.js';
import { resolveInteractionProfile } from './profiles.js';

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
};

export class SessionManager {
  private sessions = new Map<string, SessionState>();

  async startSession(cmd: StartSessionCommand) {
    const sessionId = cmd.session_id ?? randomUUID();
    const timeout = cmd.timeout_ms ?? 60000;

    if (!cmd.user_data_dir) {
      throw new Error('start_session requires user_data_dir for persistent browser sessions');
    }

    const persistentOptions: Parameters<typeof chromium.launchPersistentContext>[1] = {
      headless: !(cmd.headed ?? false)
    };

    if (cmd.browser_channel && cmd.browser_channel !== 'chromium') {
      persistentOptions.channel = cmd.browser_channel;
    }

    const context = await chromium.launchPersistentContext(cmd.user_data_dir, persistentOptions);
    const page = context.pages()[0] ?? await context.newPage();
    page.setDefaultTimeout(timeout);
    await page.goto(cmd.url, { waitUntil: 'domcontentloaded', timeout });

    if (cmd.wait_for) {
      await page.waitForSelector(cmd.wait_for, { timeout });
    }

    const { name: resolvedProfileName, profile } = await resolveInteractionProfile(page, 'auto');

    const state: SessionState = {
      sessionId,
      browser: undefined,
      context,
      page,
      profile: resolvedProfileName,
      responseSelector: profile.responseSelector,
      uploadInputSelector: profile.uploadInputSelector,
      uploadButtonSelector: profile.uploadButtonSelector,
      chatInputSelector: profile.chatInputSelector,
      chatSubmitSelector: profile.chatSubmitSelector,
      ownsBrowser: true,
      attachedViaCdp: false
    };

    this.sessions.set(sessionId, state);

    return {
      session_id: sessionId,
      attached_via_cdp: false,
      url: page.url(),
      title: await page.title()
    };
  }

  async connectOverCdp(cmd: ConnectOverCdpCommand) {
    const sessionId = cmd.session_id ?? randomUUID();
    console.error('[bridge] connectOverCdp start', {
      sessionId,
      cdpUrl: cmd.cdp_url,
      pageUrlContains: cmd.page_url_contains
    });
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
      attachedViaCdp: true
    };

    this.sessions.set(sessionId, state);
    console.error('[bridge] connectOverCdp stored session', {
      sessionId,
      sessionsSize: this.sessions.size,
      pageUrl: page.url()
    });

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
    await page.goto(cmd.url, { waitUntil: 'domcontentloaded', timeout });
    state.page = page;
    return {
      session_id: state.sessionId,
      url: page.url(),
      title: await page.title()
    };
  }

  async probePage(cmd: ProbePageCommand) {
    console.error('[bridge] probePage start', {
      sessionId: cmd.session_id,
      profile: cmd.profile
    });
    const state = this.getSession(cmd.session_id);
    const timeout = cmd.timeout_ms ?? 5000;
    const page = state.page;
    const { name: resolvedProfileName, profile } = await resolveInteractionProfile(page, cmd.profile ?? state.profile ?? 'auto');

    console.error('[bridge] probePage state', {
      sessionId: state.sessionId,
      pageUrl: state.page.url(),
      pageClosed: state.page.isClosed(),
      storedProfile: state.profile
    });

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
    const chatInput = page.locator(chatInputSelector).first();
    const inputCount = await chatInput.count();
    const chatInputFound = inputCount > 0;
    const chatInputVisible = chatInputFound ? await chatInput.isVisible({ timeout }).catch(() => false) : false;

    const submitLocator = state.chatSubmitSelector ? page.locator(state.chatSubmitSelector).first() : null;
    const submitCount = submitLocator ? await submitLocator.count().catch(() => 0) : 0;
    const chatSubmitFound = submitCount > 0;

    return {
      session_id: state.sessionId,
      browser_connected: true,
      page_open: true,
      url: page.url(),
      profile: state.profile ?? 'auto',
      chat_input_found: chatInputFound,
      chat_input_visible: chatInputVisible,
      chat_submit_found: chatSubmitFound,
      ready: chatInputFound && chatInputVisible
    };
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

  private async findVisibleChatComposer(page: Page, timeoutMs: number, preferredSelector?: string): Promise<Locator> {
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

      await page.waitForTimeout(100);
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
    const timeout = cmd.timeout_ms ?? 30000;
    const inputSelector = cmd.input_selector ?? state.chatInputSelector;
    const submitSelector = cmd.submit_selector ?? state.chatSubmitSelector;
    const composer = await this.findVisibleChatComposer(state.page, timeout, inputSelector);

    await this.writeChatComposerText(state.page, composer, cmd.text);

    if (submitSelector) {
      const submit = state.page.locator(submitSelector).first();
      if (await submit.count()) {
        await submit.click({ timeout });
        await this.waitForComposerToClear(composer, Math.min(timeout, 5000)).catch(() => {});
        return { session_id: state.sessionId, sent: true, method: 'click', text: cmd.text };
      }
    }

    const beforeSendText = await composer.evaluate((el) => {
      if ((el as HTMLElement).isContentEditable) {
        return (el.textContent ?? '').trim();
      }
      if (el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement) {
        return el.value.trim();
      }
      return (el.textContent ?? '').trim();
    }).catch(() => '');

    await composer.press('Enter', { timeout });
    await this.waitForComposerToClear(composer, Math.min(timeout, 5000)).catch(() => {});

    const afterSendText = await composer.evaluate((el) => {
      if ((el as HTMLElement).isContentEditable) {
        return (el.textContent ?? '').trim();
      }
      if (el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement) {
        return el.value.trim();
      }
      return (el.textContent ?? '').trim();
    }).catch(() => '');

    return {
      session_id: state.sessionId,
      sent: beforeSendText.length > 0 && afterSendText.length === 0,
      method: 'enter',
      text: cmd.text
    };
  }

  async uploadFile(cmd: UploadFileCommand) {
    const state = this.getSession(cmd.session_id);
    const timeout = cmd.timeout_ms ?? 30000;
    const inputSelector = cmd.input_selector ?? state.uploadInputSelector ?? 'input[type="file"]';

    const input = state.page.locator(inputSelector).first();
    if (await input.count()) {
      await input.setInputFiles(cmd.file_path, { timeout });
      return { session_id: state.sessionId, uploaded: cmd.file_path, via: 'input' };
    }

    const triggerSelector = cmd.button_selector ?? state.uploadButtonSelector;
    if (!triggerSelector) {
      throw new Error(`No file input found for session ${state.sessionId}`);
    }

    const trigger = state.page.locator(triggerSelector).first();
    const chooserPromise = state.page.waitForEvent('filechooser', { timeout });
    await trigger.click({ timeout });
    const chooser = await chooserPromise;
    await chooser.setFiles(cmd.file_path);

    return { session_id: state.sessionId, uploaded: cmd.file_path, via: 'filechooser' };
  }

  async readResponse(cmd: ReadResponseCommand) {
    const state = this.getSession(cmd.session_id);
    const timeout = cmd.timeout_ms ?? 120000;
    const idleMs = cmd.idle_ms ?? 1200;
    const selector = cmd.response_selector ?? state.responseSelector;

    await state.page.waitForSelector(selector, { timeout });
    await this.waitForStableResponse(state.page, selector, timeout, idleMs);

    const texts = await state.page.locator(selector).evaluateAll((nodes) =>
      nodes
        .map((node) => (node.textContent ?? '').trim())
        .filter((text) => text.length > 0)
    );

    const text = texts.length ? texts[texts.length - 1] : '';

    return {
      session_id: state.sessionId,
      response: text,
      response_count: texts.length
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

  private getSession(sessionId: string): SessionState {
    console.error('[bridge] getSession', {
      sessionId,
      sessionsSize: this.sessions.size,
      known: Array.from(this.sessions.keys())
    });
    const state = this.sessions.get(sessionId);
    if (!state) {
      console.error('[bridge] getSession MISS', { sessionId });
      throw new Error(`Unknown session_id: ${sessionId}`);
    }
    console.error('[bridge] getSession HIT', {
      sessionId,
      pageUrl: state.page.url(),
      pageClosed: state.page.isClosed()
    });
    return state;
  }

  private async waitForComposerToClear(composer: Locator, timeoutMs: number): Promise<void> {
    const deadline = Date.now() + timeoutMs;

    while (Date.now() < deadline) {
      const state = await composer.evaluate((el) => ({
        text: (el.textContent ?? '').trim(),
        ariaBusy: el.getAttribute('aria-busy'),
        isContentEditable: (el as HTMLElement).isContentEditable
      })).catch(() => null);

      if (!state) {
        return;
      }

      if (state.isContentEditable && state.text.length === 0) {
        return;
      }

      await composer.page().waitForTimeout(100);
    }
  }

  private async waitForStableResponse(page: Page, selector: string, timeoutMs: number, idleMs: number) {
    const start = Date.now();
    let last = '';
    let stableSince = Date.now();

    while (Date.now() - start < timeoutMs) {
      const current = await page.locator(selector).last().textContent().catch(() => null);
      const value = (current ?? '').trim();

      if (value !== last) {
        last = value;
        stableSince = Date.now();
      }

      if (value.length > 0 && Date.now() - stableSince >= idleMs) {
        return;
      }

      await page.waitForTimeout(250);
    }

    throw new Error(`Timed out waiting for stable response for selector: ${selector}`);
  }
}
