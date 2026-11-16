from app.models import User, UserRepository


def test_display_name_falls_back_to_email():
    user = User(id=1, name="", email="ada@example.com")
    assert user.display_name() == "ada"


def test_repository_roundtrip():
    repo = UserRepository()
    repo.save(User(id=7, name="grace", email="g@example.com"))
    assert repo.find(7).name == "grace"
