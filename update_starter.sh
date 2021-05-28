home=`cat rm_material/manifest.json | jq -r .posts.folders.MRKR.folders.Starter.documents.Home.id`
logo=`cat rm_material/manifest.json | jq -r .posts.folders.MRKR.folders.Starter.documents.Logo.id`
sample_notebook=`cat rm_material/manifest.json | jq -r '.posts.folders.MRKR.folders.Starter.folders.Posts.documents."Sample Notebook".id'`
boxes_and_arrows=`cat rm_material/manifest.json | jq -r '.posts.folders.MRKR.folders.Starter.folders.Posts.folders."Folders Work Too".documents."Boxes + Arrows".id'`
pyth_theorem=`cat rm_material/manifest.json | jq -r '.posts.folders.MRKR.folders.Starter.folders.Posts.folders."Folders Work Too".documents."Pythagorean Theorem".id'`

rm -r starter/
mkdir -p starter
cp "rm_material/zip/$home.zip" starter/Home.zip
cp "rm_material/zip/$logo.zip" starter/Logo.zip
mkdir -p starter/Posts
cp "rm_material/zip/$sample_notebook.zip" "starter/Posts/Sample Notebook.zip"
mkdir -p "starter/Posts/Folders Work Too"
cp "rm_material/zip/$boxes_and_arrows.zip" "starter/Posts/Folders Work Too/Boxes + Arrows.zip"
cp "rm_material/zip/$pyth_theorem.zip" "starter/Posts/Folders Work Too/Pythagorean Theorem.zip"

