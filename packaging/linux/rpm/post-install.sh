# Apply the USB permission rule right away, so a printer that is already
# plugged in works without unplugging it or rebooting.
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || :
    udevadm trigger --subsystem-match=usbmisc --action=change || :
fi
