from dataclasses import dataclass, field
from datetime import datetime


@dataclass
class User:
    """A registered user."""

    id: int
    name: str
    email: str
    created_at: datetime = field(default_factory=datetime.now)
    tags: list[str] = field(default_factory=list)

    def display_name(self) -> str:
        # Fall back to the email prefix when no name is set
        if not self.name:
            return self.email.split("@")[0]
        return self.name.title()

    def add_tag(self, tag: str) -> None:
        if tag not in self.tags:
            self.tags.append(tag)


class UserRepository:
    def __init__(self):
        self._users: dict[int, User] = {}

    def save(self, user: User) -> User:
        self._users[user.id] = user
        return user

    def find(self, user_id: int) -> User | None:
        return self._users.get(user_id)

    def all(self) -> list[User]:
        return sorted(self._users.values(), key=lambda u: u.created_at)


def find_by_prefix(repo, prefix):
    return [u for u in repo.all() if u.name.startswith(prefix)]
