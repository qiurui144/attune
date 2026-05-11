"""watchdog 目录监听：监控绑定目录的文件变更"""

import json
import logging
from pathlib import Path

from watchdog.events import FileCreatedEvent, FileModifiedEvent, FileSystemEventHandler
from watchdog.observers import Observer

logger = logging.getLogger(__name__)


class FileChangeHandler(FileSystemEventHandler):
    """文件变更事件处理器"""

    def __init__(self, callback: callable, file_types: list[str]) -> None:  # type: ignore[type-arg]
        self.callback = callback
        self.suffixes = {f".{ft.lower().lstrip('.')}" for ft in file_types}

    def _should_process(self, path: str) -> bool:
        return Path(path).suffix.lower() in self.suffixes

    def on_created(self, event: FileCreatedEvent) -> None:  # type: ignore[override]
        if not event.is_directory and self._should_process(event.src_path):
            logger.debug("File created: %s", event.src_path)
            self.callback(event.src_path, "created")

    def on_modified(self, event: FileModifiedEvent) -> None:  # type: ignore[override]
        if not event.is_directory and self._should_process(event.src_path):
            logger.debug("File modified: %s", event.src_path)
            self.callback(event.src_path, "modified")


class DirectoryWatcher:
    """管理多个目录的文件监听"""

    def __init__(self, callback: callable) -> None:  # type: ignore[type-arg]
        self.callback = callback
        self.observer = Observer()
        self._watches: dict[str, object] = {}

    def watch(self, dir_path: str, recursive: bool = True, file_types: list[str] | None = None) -> None:
        """添加目录监听"""
        if dir_path in self._watches:
            return
        if not Path(dir_path).is_dir():
            logger.warning("Directory not found: %s", dir_path)
            return

        types = file_types or ["md", "txt", "pdf", "docx", "py", "js"]
        handler = FileChangeHandler(self.callback, types)
        watch = self.observer.schedule(handler, dir_path, recursive=recursive)
        self._watches[dir_path] = watch
        logger.info("Watching directory: %s (recursive=%s, types=%s)", dir_path, recursive, types)

    def unwatch(self, dir_path: str) -> None:
        """移除目录监听"""
        watch = self._watches.pop(dir_path, None)
        if watch:
            self.observer.unschedule(watch)  # type: ignore[arg-type]
            logger.info("Unwatched directory: %s", dir_path)

    def start(self) -> None:
        self.observer.start()
        logger.info("Directory watcher started")

    def stop(self) -> None:
        self.observer.stop()
        self.observer.join(timeout=5)
        logger.info("Directory watcher stopped")

    def load_from_db(self, directories: list[dict]) -> None:
        """从数据库加载绑定的目录并开始监听"""
        for d in directories:
            file_types = json.loads(d.get("file_types", '["md","txt"]'))
            self.watch(d["path"], recursive=bool(d.get("recursive", 1)), file_types=file_types)
