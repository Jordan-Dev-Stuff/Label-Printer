# Drop the USB permission rule's effect once the package is removed
# ($1 is 0 on removal, 1 or more on upgrade).
if [ "$1" -eq 0 ] && command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || :
    udevadm trigger --subsystem-match=usbmisc --action=change || :
fi
