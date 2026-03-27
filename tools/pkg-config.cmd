@echo off
setlocal EnableExtensions
python "%~dp0pkg-config.py" %*
set "exit_code=%ERRORLEVEL%"
exit /b %exit_code%
