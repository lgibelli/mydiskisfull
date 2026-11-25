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


def _until(what, get, secs):
    end = time.time() + secs
    while True:
        try:
            found = get()
        except comtypes.COMError:
            found = None  # the page is being drawn again
        if found:
            return found
        if time.time() > end:
            sys.exit(f'timed out waiting for {what}')
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
    """The button labelled `label` (its text, spaces aside)."""
    cond = uia.CreatePropertyCondition(U.UIA_ControlTypePropertyId, U.UIA_ButtonControlTypeId)
    return _until(f'the {label} button', lambda: next(
        (b for b in _elements(window, cond) if (b.CurrentName or '').strip() == label
         and not b.CurrentIsOffscreen and b.CurrentIsEnabled), None), secs)


def press(window, label, secs=60):
    button = find(window, label, secs)
    button.GetCurrentPattern(U.UIA_InvokePatternId) \
        .QueryInterface(U.IUIAutomationInvokePattern).Invoke()
    print(f'pressed {label}')


def named(window, test):
    """The names of what the window shows that pass `test`."""
    try:
        return [n for e in _elements(window, uia.CreateTrueCondition())
                if (n := (e.CurrentName or '').strip()) and test(n)]
    except comtypes.COMError:
        return []


def shot(path):
    ImageGrab.grab(all_screens=True).save(path)
