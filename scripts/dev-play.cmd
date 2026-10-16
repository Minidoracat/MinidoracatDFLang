@echo off
rem DFLang dev launcher: build this repo, mount dist/ into the game, start it,
rem and restore everything after the game is closed.
rem Optional: dev-play.cmd --lang zh-Hans
cd /d "%~dp0.."
uv run scripts/dev_game.py play %*
pause
