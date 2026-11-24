@echo off
rem Launch from a private copy so the running MCP server never locks the build.
rem Use it as the MCP command:
rem   "command": "cmd", "args": ["/c", "<repo>\\tools\\qucs-s-rs-mcp.cmd"]
rem Extra arguments are passed on after "mcp".
setlocal
set "SRC=%~dp0..\target\release\qucs-t.exe"
if not exist "%SRC%" set "SRC=%~dp0..\target\debug\qucs-t.exe"
if not exist "%SRC%" (
    echo qucs-t.exe not found. Run cargo build before starting MCP. 1>&2
    exit /b 1
)
set "DIR=%LOCALAPPDATA%\qucs-t\mcp-bin"
if not exist "%DIR%" mkdir "%DIR%" >nul 2>&1
rem drop copies no longer in use (running ones are locked and stay)
del /q "%DIR%\qucs-s-rs-*.exe" >nul 2>&1
set "DST=%DIR%\qucs-s-rs-%RANDOM%%RANDOM%.exe"
copy /y "%SRC%" "%DST%" >nul 2>&1 || set "DST=%SRC%"
"%DST%" mcp %*
