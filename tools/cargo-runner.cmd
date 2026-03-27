@echo off
setlocal EnableExtensions

set "ROOT=%~dp0.."
set "PATH=%ROOT%\vcpkg_installed\x64-windows\bin;%ROOT%\vcpkg_installed\x64-windows\debug\bin;%PATH%"

set "EXE=%~1"
shift
"%EXE%" %*
set "exit_code=%ERRORLEVEL%"
exit /b %exit_code%
