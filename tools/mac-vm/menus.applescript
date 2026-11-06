-- menus.applescript (for run.sh): opens every menu of Rookparcel's menu bar through System
-- Events and lists its items with their state and key into $RP_SHOTS/menus.txt, then runs
-- items by clicking them (accessibility actions, which reach the app even while the VM's screen
-- is locked, unlike typed keys): the View toggles, a header click, a row selection with a
-- Torrent menu item, and items that log "not yet". After each step it keeps the snapshot of the
-- moment as <step>.png and <step>.txt; steps.txt says what each step did or why it failed.
--
-- The items that need a key window (Find and the Edit items in the search field, Minimize,
-- Zoom, Full Screen) are tried too; they are disabled while the screen is locked.
--
--   RP_SHOTS=~/rp/B/shots osascript menus.applescript   (run.sh sets RP_SHOTS)

on logStep(t)
	set shots to system attribute "RP_SHOTS"
	do shell script "echo " & quoted form of t & " >> " & quoted form of (shots & "/steps.txt")
end logStep

on keep(stepName)
	set shots to system attribute "RP_SHOTS"
	delay 2.5
	do shell script "cp " & quoted form of (shots & "/window-1.png") & " " & quoted form of (shots & "/" & stepName & ".png") & " 2>/dev/null; cp " & quoted form of (shots & "/windows.txt") & " " & quoted form of (shots & "/" & stepName & ".txt") & " 2>/dev/null; true"
	my logStep("kept " & stepName)
end keep

on listMenu(m, indent)
	set out to ""
	tell application "System Events"
		repeat with i in menu items of m
			set n to name of i
			if n is missing value then
				set out to out & indent & "----" & linefeed
			else
				set e to enabled of i
				set k to ""
				try
					set k to value of attribute "AXMenuItemCmdChar" of i
					if k is missing value then set k to ""
				end try
				set ln to indent & n
				if not e then set ln to ln & "  (disabled)"
				if k is not "" then set ln to ln & "  [" & k & "]"
				set out to out & ln & linefeed
				try
					if exists menu 1 of i then set out to out & my listMenu(menu 1 of i, indent & "    ")
				end try
			end if
		end repeat
	end tell
	return out
end listMenu

-- clicks the item (a name or an index) of the menu bar's menu number `menuIndex`
on clickItem(menuIndex, itemName)
	repeat with attempt from 1 to 5
		try
			tell application "System Events" to tell process "Rookparcel" to get menu bar 1
			exit repeat
		on error
			-- the accessibility tree can be away for a moment after the window changed
			delay 1
		end try
	end repeat
	try
		tell application "System Events" to tell process "Rookparcel"
			set b to menu bar item menuIndex of menu bar 1
			click b
			delay 0.4
			set i to menu item itemName of menu 1 of b
			set n to name of i
			if enabled of i then
				click i
				my logStep("clicked " & n)
			else
				click b
				my logStep("disabled: " & n)
			end if
		end tell
	on error e
		my logStep("failed: " & itemName & ": " & e)
	end try
	delay 0.7
end clickItem

on nameOf(menuIndex, k)
	repeat with attempt from 1 to 5
		try
			tell application "System Events" to tell process "Rookparcel"
				return name of menu item k of menu 1 of menu bar item menuIndex of menu bar 1
			end tell
		on error e
			delay 1
		end try
	end repeat
	my logStep("no name for item " & k & " of menu " & menuIndex)
	return k
end nameOf

set shots to system attribute "RP_SHOTS"
tell application "System Events"
	set report to ""
	tell process "Rookparcel"
		repeat with b in menu bar items of menu bar 1
			set t to name of b
			click b
			delay 0.4
			set report to report & "== " & t & linefeed & my listMenu(menu 1 of b, "  ")
			-- a second click closes it (Escape, a typed key, does not reach a locked session)
			click b
			delay 0.3
		end repeat
	end tell
end tell
do shell script "printf '%s' " & quoted form of report & " > " & quoted form of (shots & "/menus.txt")

-- menu bar items: 1 Apple, 2 Rookparcel, 3 File, 4 Edit, 5 View, 6 Torrent, 7 Options, 8 Window, 9 Help
-- the View toggles: off, a snapshot, on again (their titles follow the state)
repeat with k in {1, 4, 5, 6}
	my clickItem(5, my nameOf(5, k))
	my keep("view-" & k)
	my clickItem(5, my nameOf(5, k))
end repeat

-- sort: click the Size column's header twice (ascending, then descending); select a row, so
-- the Torrent menu's items come alive (Start logs "not yet")
try
	tell application "System Events" to tell process "Rookparcel"
		set els to entire contents of window 1
	end tell
	set sizeHeader to missing value
	set theTable to missing value
	repeat with e in els
		tell application "System Events"
			set c to class of e
			if c is button and sizeHeader is missing value then
				try
					if (title of e) is "Size" or (description of e) is "Size" then set sizeHeader to contents of e
				end try
			else if c is table and theTable is missing value then
				if (count of columns of e) > 5 then set theTable to contents of e
			end if
		end tell
	end repeat
	if sizeHeader is missing value then
		my logStep("no Size header found")
	else
		tell application "System Events" to click sizeHeader
		my keep("sorted-size")
		tell application "System Events" to click sizeHeader
		my keep("sorted-size-desc")
	end if
	if theTable is missing value then
		my logStep("no table found")
	else
		tell application "System Events" to set selected of row 3 of theTable to true
		my keep("selected")
		my clickItem(6, 4)
	end if
on error e
	my logStep("sort or selection failed: " & e)
end try

-- items not built yet: they log "not yet"
my clickItem(3, 1)
my clickItem(7, 1)
tell application "System Events" to tell process "Rookparcel"
	set helpLast to name of last menu item of menu 1 of menu bar item 9 of menu bar 1
end tell
my clickItem(9, helpLast)

-- the items that need a key window: Find (Edit's 10th), Full Screen (View's last), Minimize
-- (Window's first)
tell application "System Events" to tell process "Rookparcel"
	set fullName to name of last menu item of menu 1 of menu bar item 5 of menu bar 1
end tell
my clickItem(4, 10)
my clickItem(5, fullName)
delay 2
my clickItem(8, 1)

-- Hide (the app menu's), then show it again
my clickItem(2, 9)
delay 1
tell application "System Events" to set v to visible of process "Rookparcel"
my logStep("visible after Hide: " & v)
tell application "System Events" to set visible of process "Rookparcel" to true
delay 1
tell application "System Events" to set v to visible of process "Rookparcel"
my logStep("visible after showing it: " & v)
my keep("end")
