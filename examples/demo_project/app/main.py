import os
import sys

from app.models import User, UserRepository

MAX_USERS = 100
DEBUG = os.environ.get("DEBUG", "0") == "1"


# entry point
def main(argv: list[str]) -> int:
    repo = UserRepository()
    for i, name in enumerate(argv[1:], start=1):
        user = repo.save(User(id=i, name=name, email=f"{name.lower()}@example.com"))
        user.add_tag("cli")
        if DEBUG:
            print(f"created {user!r}")

    for user in repo.all():
        print(user.display_name(), len(user.tags))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
