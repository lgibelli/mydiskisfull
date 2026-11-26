"""The app's window used as a person uses it, through Windows' UI Automation
(WebView2 shows the page's controls to it, buttons by their labels), and
screenshots of the screen. For the live checks (live_update.py); comtypes
and Pillow are installed by the workflow."""
import sys, time
import comtypes.client
from PIL import ImageGrab

comtypes.client.GetModule('UIAutomationCore.dll')
from comtypes.gen import UIAutomationClient as U  # noqa: E402

uia = comtypes.client.CreateObject(U.CUIAutomation, interface=U.IUIAutomation)


def _until(what, get, secs, seen=lambda: ''):
    end = time.time() + secs
    while True:
        try:
            found = get()
        except (comtypes.COMError, ValueError):
            found = None  # the page is being drawn again, or the window just closed
        if found:
            return found
        if time.time() > end:
            sys.exit(f'timed out waiting for {what}{seen()}')
        time.sleep(1)


def wait_window(title, secs=120):
    """The top-level window titled `title`."""
    cond = uia.CreateAndCondition(
        uia.CreatePropertyCondition(U.UIA_NamePropertyId, title),
        uia.CreatePropertyCondition(U.UIA_ControlTypePropertyId, U.UIA_WindowControlTypeId))
    return _until(f'the {title} window',
                  lambda: uia.GetRootElement().FindFirst(U.TreeScope_Children, cond), secs)


def _elements(window, cond):
    found = window.FindAll(U.TreeScope_Descendants, cond)
    return [found.GetElement(i) for i in range(found.Length)]


def find(window, label, secs=60):
    """The button labelled `label` (its text, spaces aside), enabled."""
    cond = uia.CreatePropertyCondition(U.UIA_ControlTypePropertyId, U.UIA_ButtonControlTypeId)
    return _until(f'the {label} button', lambda: next(
        (b for b in _elements(window, cond) if (b.CurrentName or '').strip() == label
         and b.CurrentIsEnabled), None), secs,
        lambda: f'; its buttons: {[b.CurrentName for b in _elements(window, cond)]}')


def press(window, label, secs=60):
    button = find(window, label, secs)
    # Scrolled to first, as a person would (one below the fold is offscreen).
    try:
        button.GetCurrentPattern(U.UIA_ScrollItemPatternId) \
            .QueryInterface(U.IUIAutomationScrollItemPattern).ScrollIntoView()
        time.sleep(1)
    except comtypes.COMError:
        pass
    button.GetCurrentPattern(U.UIA_InvokePatternId) \
        .QueryInterface(U.IUIAutomationInvokePattern).Invoke()
    print(f'pressed {label}')


def named(window, test):
    """The names of what the window shows that pass `test`."""
    try:
        return [n for e in _elements(window, uia.CreateTrueCondition())
                if (n := (e.CurrentName or '').strip()) and test(n)]
    except (comtypes.COMError, ValueError):  # ValueError: the window has just closed
        return []


def alive(window):
    """Whether the window is still there (an app installing an update hides
    it before it ends)."""
    try:
        return window.CurrentIsOffscreen is not None and bool(window.CurrentName)
    except (comtypes.COMError, ValueError):
        return False


def shot(path):
    ImageGrab.grab(all_screens=True).save(path)
